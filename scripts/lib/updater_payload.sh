#!/usr/bin/env bash
# updater_payload.sh: repack, re-sign and verify `Lucidos.app.tar.gz`, the archive
# the in-app auto-updater installs.
#
# ── WHY THIS EXISTS: the v0.19.0 incident ────────────────────────────────────
# `cargo tauri build` packs the updater tarball from the .app as the bundler
# leaves it. build-dmg.sh deliberately runs that build with APPLE_SIGNING_IDENTITY
# removed from the SUBPROCESS env (Tauri's own codesign pass skips the ~200 loose
# Mach-O files in the relocatable Postgres tree, so build-dmg.sh signs the bundle
# itself, inside-out, afterwards). Nothing ever repacked the tarball, so every
# release shipped a DMG containing a Developer ID app next to an updater payload
# containing an AD-HOC one.
#
# The v0.19.0 payload, extracted from the published release: `Signature=adhoc`,
# `TeamIdentifier=not set`, designated requirement `cdhash H"d3974ae45f…"`, and no
# `Contents/_CodeSignature` directory at all. A cdhash-anchored designated
# requirement changes with every build, and macOS TCC keys permission grants on
# code identity, so EVERY auto-update silently destroyed every grant the user had
# ever given. This is the same failure the dev engine's stable self-signed
# identity (scripts/lib/codesign.sh) exists to prevent, one layer up.
#
# ── THE UPDATER'S EXTRACTION CONTRACT (why the layout rules below are hard) ───
# tauri-plugin-updater's macOS `install_inner` does, per entry:
#     let collected_path: PathBuf = entry.path()?.iter().skip(1).collect();
#     entry.unpack(&tmp_extract_dir.join(collected_path))?;
# Three consequences, all asserted by updater_payload_assert_archive_shape:
#
#   1. Every entry must sit under exactly ONE top-level component (`Lucidos.app/`).
#      `skip(1)` strips it blindly, so an archive rooted at `Contents/` would have
#      `Contents` stripped and install a bundle with no `Contents` directory.
#   2. NO hard-link entries. `Entry::unpack` resolves a hard link's target against
#      the process CWD rather than the extraction root, so a hard link either
#      fails the install or links to the wrong file. The Rust `tar` crate Tauri
#      packs with never EMITS hard links (it writes each inode again), which is
#      why this has never bitten; `bsdtar`, which we repack with, detects and
#      emits them. So the hazard is one the repack introduces, and the assertion
#      is what keeps it from ever shipping.
#   3. NO AppleDouble `._` entries. They are not in the bundle's CodeResources
#      seal, so extracting them alongside the real files makes `codesign --verify`
#      report added resources and macOS refuses to launch the app.
#
# Symlinks are fine (the tar crate unpacks them as symlinks) and file modes carry
# through, so the exec bits survive.
#
# Verified against the real consumer rather than assumed: a bundle signed
# inside-out, packed the way updater_payload_repack packs it, then extracted by a
# throwaway program running that exact `skip(1)` + `Entry::unpack` loop with the
# same `tar` crate, still passes `codesign --verify --deep --strict`, keeps its
# symlink as a symlink and keeps its exec bits.
#
# ── Public-mirror safety ─────────────────────────────────────────────────────
# `build-dmg.sh --release-build` is a legitimate public path, and this lib is on
# its critical path, so this file must stay OUT of RELEASE_TREE_EXCLUDE_PATHS and
# is sourced unconditionally. Contrast release_signing.sh / release_events.sh,
# which are stripped and therefore may only be sourced behind an `if [ -f … ]`.
#
# macOS-only in practice (codesign, and bsdtar's flags), which is fine: it is only
# ever reached from build-dmg.sh, which refuses to run anywhere else.
#
# Unit tests: scripts/lib/updater_payload_test.sh.

# The one `cargo tauri signer sign` call site (tauri_signer_sign_file).
# shellcheck source=scripts/lib/tauri_signing_key.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/tauri_signing_key.sh"

# ── Pure predicates over codesign output ─────────────────────────────────────
# Split out as text functions so the gate is unit-testable against captured real
# `codesign` output. Producing a genuine Developer ID signature needs the release
# certificate and a 40-minute build, so an end-to-end test could only ever cover
# the REJECT direction; these cover both.

# updater_payload_dr_is_developer_id <codesign -d -r- output>: zero when the
# designated requirement is anchored to an Apple-issued Developer ID certificate,
# i.e. it survives a rebuild and keeps the user's TCC grants.
#
# The real shape, taken from the (correct) v0.18.2 DMG payload with the team
# identifier generalized:
#   designated => identifier "com.lucidos.app" and anchor apple generic
#     and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */
#     and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */
#     and certificate leaf[subject.OU] = AB12CD34EF
#
# Both halves are required on purpose. `anchor apple generic` alone would also
# accept a requirement anchored at an Apple root with no team, and the subject.OU
# term alone would accept a hand-written requirement with no anchor.
updater_payload_dr_is_developer_id() {
    local text="$1"
    case "$text" in *"anchor apple generic"*) ;; *) return 1 ;; esac
    case "$text" in *"certificate leaf[subject.OU] = "*) ;; *) return 1 ;; esac
    return 0
}

# updater_payload_dr_is_cdhash_pinned <codesign -d -r- output>: zero when the
# designated requirement names only a code-directory hash and no certificate.
# This is EXACTLY what shipped in v0.19.0, and it is worth telling apart from
# "some other wrong requirement" so the refusal can say what actually happened.
updater_payload_dr_is_cdhash_pinned() {
    local text="$1"
    case "$text" in *certificate*) return 1 ;; esac
    case "$text" in *'cdhash H"'*) return 0 ;; esac
    return 1
}

# updater_payload_team_id_is_set <codesign -dv output>: zero when the signature
# carries a real Team Identifier. codesign prints the literal `not set` for an
# ad-hoc signature and for a self-signed certificate (which has no team), so the
# absence of a team is the cheapest single proof that a payload is not Developer
# ID signed.
updater_payload_team_id_is_set() {
    local text="$1" line value
    line="$(printf '%s\n' "$text" | grep -E '^TeamIdentifier=' | head -1 || true)"
    [ -n "$line" ] || return 1
    value="${line#TeamIdentifier=}"
    case "$value" in ""|"not set") return 1 ;; esac
    return 0
}

# ── Archive shape ────────────────────────────────────────────────────────────

# updater_payload_archive_top_level <tarball>: print the archive's single
# top-level path component. Non-zero (with a reason) when the archive is empty or
# has more than one, which is the case `skip(1)` cannot survive.
updater_payload_archive_top_level() {
    local tarball="$1" tops count
    tops="$(tar -tzf "$tarball" 2>/dev/null | sed -e 's#/.*##' -e '/^$/d' | sort -u)" \
        || { echo "ERROR: could not list $tarball" >&2; return 1; }
    if [ -z "$tops" ]; then
        echo "ERROR: $tarball contains no entries." >&2
        return 1
    fi
    count="$(printf '%s\n' "$tops" | wc -l | tr -d '[:space:]')"
    if [ "$count" != "1" ]; then
        echo "ERROR: $tarball has $count top-level entries; the updater strips the first path component, so it needs exactly one:" >&2
        printf '%s\n' "$tops" | sed 's/^/       /' >&2
        return 1
    fi
    printf '%s' "$tops"
}

# updater_payload_assert_archive_shape <tarball> [<expected-top-level>]: enforce
# the three layout rules the updater's extraction imposes (see the header block).
# Prints the resolved top-level component on success.
updater_payload_assert_archive_shape() {
    local tarball="$1" expected="${2:-}" top listing offenders

    [ -f "$tarball" ] || { echo "ERROR: no updater tarball at $tarball" >&2; return 1; }
    top="$(updater_payload_archive_top_level "$tarball")" || return 1

    case "$top" in
        *.app) ;;
        *) echo "ERROR: $tarball is rooted at '$top', which is not a .app bundle. The updater unpacks the contents of that directory over the installed app." >&2
           return 1 ;;
    esac
    if [ -n "$expected" ] && [ "$top" != "$expected" ]; then
        echo "ERROR: $tarball is rooted at '$top' but the signed bundle is '$expected'." >&2
        return 1
    fi

    # `tar -tvzf` prints the entry type as the first character of the mode field:
    # 'h' for a hard link, 'l' for a symlink, 'd' for a directory. Only 'h' is
    # fatal (see rule 2 in the header).
    listing="$(tar -tvzf "$tarball" 2>/dev/null)" \
        || { echo "ERROR: could not list $tarball" >&2; return 1; }
    offenders="$(printf '%s\n' "$listing" | grep '^h' || true)"
    if [ -n "$offenders" ]; then
        echo "ERROR: $tarball contains hard-link entries, which tauri-plugin-updater cannot unpack (it resolves the link target against the process CWD, not the extraction root):" >&2
        printf '%s\n' "$offenders" | sed 's/^/       /' >&2
        return 1
    fi

    offenders="$(tar -tzf "$tarball" 2>/dev/null | grep -E '(^|/)\._' || true)"
    if [ -n "$offenders" ]; then
        echo "ERROR: $tarball contains AppleDouble (._) entries. They are outside the bundle's CodeResources seal, so the extracted app would fail codesign verification. Pack with COPYFILE_DISABLE=1." >&2
        printf '%s\n' "$offenders" | sed 's/^/       /' >&2
        return 1
    fi

    printf '%s' "$top"
}

# ── Repack + re-sign ─────────────────────────────────────────────────────────

# updater_payload_repack <app> <tarball>: rebuild <tarball> from the (already
# signed) <app>, replacing it in place only after the new archive has proved
# itself. The proof is a full round trip: extract what was just written and run
# `codesign --verify --deep --strict` on the result. A repack that silently broke
# the seal would be worse than the bug it fixes, because the installed app would
# refuse to launch instead of merely losing its permissions.
#
# `tar` and not `ditto -c -k`: ditto writes a zip, and the updater reads a gzip
# tar.
#
# Two packing flags, and both are about keeping the archive in the SAME shape
# Tauri's own tar-rs writer has produced for every release so far:
#
#   COPYFILE_DISABLE=1  stops an Apple tar from turning extended attributes and
#     resource forks into AppleDouble `._` sidecar entries (rule 3). Measured as
#     a no-op on libarchive 3.7.4, which stores xattrs as `SCHILY.xattr.*` pax
#     attributes instead, but it is the documented knob in tar(1) and it costs
#     nothing to keep for an older tar that does use AppleDouble.
#   --no-xattrs  is what actually keeps the xattrs out, and with them every pax
#     extended header: macOS stamps `com.apple.provenance` on essentially every
#     file, so without this the archive gains a `PaxHeaders/…` entry per file.
#     The tar crate does consume those correctly (checked, and the round trip
#     below would catch it if it did not), but host-local provenance metadata has
#     no business in a shipped payload, and an archive with no pax headers at all
#     is the shape the updater has always been fed.
#
# The temp archive is written NEXT to the destination so the replacement is a
# rename on the same filesystem, and under a suffix that `*.app.tar.gz` globs do
# not match, so a crashed repack cannot leave a decoy for the staging step to
# find.
updater_payload_repack() {
    local app="$1" tarball="$2"
    local parent base tarball_dir tmp_archive verify_dir top

    [ -d "$app" ] || { echo "ERROR: no .app bundle at $app to repack from" >&2; return 1; }
    [ -f "$tarball" ] || { echo "ERROR: no updater tarball at $tarball to replace" >&2; return 1; }
    command -v codesign >/dev/null 2>&1 \
        || { echo "ERROR: codesign not found; cannot verify the repacked updater payload." >&2; return 1; }

    parent="$(cd "$(dirname "$app")" && pwd)" || return 1
    base="$(basename "$app")"
    tarball_dir="$(cd "$(dirname "$tarball")" && pwd)" || return 1
    tarball="$tarball_dir/$(basename "$tarball")"
    tmp_archive="$tarball.repack.tmp"

    rm -f "$tmp_archive"
    if ! (cd "$parent" && COPYFILE_DISABLE=1 tar --no-xattrs -czf "$tmp_archive" "$base"); then
        rm -f "$tmp_archive"
        echo "ERROR: failed to pack $app into $tmp_archive (if tar rejected --no-xattrs, see the packing-flags note above before dropping it)." >&2
        return 1
    fi

    if ! top="$(updater_payload_assert_archive_shape "$tmp_archive" "$base")"; then
        rm -f "$tmp_archive"
        return 1
    fi

    verify_dir="$(mktemp -d)" || { rm -f "$tmp_archive"; return 1; }
    if ! tar -xzf "$tmp_archive" -C "$verify_dir"; then
        rm -rf "$verify_dir"; rm -f "$tmp_archive"
        echo "ERROR: the repacked archive $tmp_archive could not be extracted." >&2
        return 1
    fi
    if ! codesign --verify --deep --strict --verbose=2 "$verify_dir/$top"; then
        rm -rf "$verify_dir"; rm -f "$tmp_archive"
        echo "ERROR: the repacked updater payload does not survive a round trip: the extracted bundle fails codesign --verify --deep --strict. Refusing to ship an archive that installs an app macOS will not launch." >&2
        return 1
    fi
    rm -rf "$verify_dir"

    mv -f "$tmp_archive" "$tarball" || {
        rm -f "$tmp_archive"
        echo "ERROR: could not replace $tarball with the repacked archive." >&2
        return 1
    }
    return 0
}

# updater_payload_resign <tarball>: regenerate <tarball>.sig over the repacked
# bytes. Shipping the pre-repack signature next to repacked bytes would make
# EVERY updater reject the update, so this is not optional once a repack has
# happened.
#
# The "changed" assertion is the load-bearing half. A signer that exits 0 without
# writing anything, or a key that resolves to something inert, would otherwise
# leave the stale .sig sitting there looking healthy. There is no way to verify
# the signature itself: `cargo tauri signer` has only `sign` and `generate`, and
# the release preflight's throwaway test-sign (release_signing.sh) is what proves
# the key is capable at all.
updater_payload_resign() {
    local tarball="$1" sig before after
    sig="$tarball.sig"
    [ -f "$tarball" ] || { echo "ERROR: no updater tarball at $tarball to sign" >&2; return 1; }

    # An `if`, not `[ -f … ] && before=…`: as a standalone statement the latter
    # returns 1 when the file is absent, which trips a caller's `set -e`. Every
    # caller today uses `|| die`, so this is a landmine rather than a live bug.
    before=""
    if [ -f "$sig" ]; then
        before="$(cat "$sig")"
    fi

    tauri_signer_sign_file "$tarball" || {
        echo "ERROR: could not re-sign $tarball with the updater key." >&2
        return 1
    }
    [ -s "$sig" ] || { echo "ERROR: the updater signer produced no $sig" >&2; return 1; }

    after="$(cat "$sig")"
    if [ -n "$before" ] && [ "$before" = "$after" ]; then
        echo "ERROR: $sig is byte-identical to the signature that existed before the repack, so nothing was actually re-signed. The updater would reject the new bytes." >&2
        return 1
    fi
    return 0
}

# ── The publish gate ─────────────────────────────────────────────────────────

# updater_payload_assert_developer_id <tarball> [<label>]: refuse a tarball whose
# payload is not Developer ID signed. This is the check that would have caught
# v0.19.0 before it was published, and it is deliberately derived from the BYTES
# every time rather than from a recorded verdict, so no staged manifest, restamp
# or re-fold can launder an unsigned payload into a signed-looking one.
#
# Three questions, all answered from the extracted bundle:
#   • does it still verify at all (a corrupt or resealed archive fails here);
#   • is its designated requirement anchored to a Developer ID certificate
#     rather than pinned to a cdhash;
#   • does it carry a Team Identifier.
updater_payload_assert_developer_id() {
    local tarball="$1" label="${2:-$1}"
    local dir top app dr dv rc=0

    command -v codesign >/dev/null 2>&1 \
        || { echo "ERROR: codesign not found; cannot verify the updater payload in $label." >&2; return 1; }
    top="$(updater_payload_assert_archive_shape "$tarball")" || return 1

    dir="$(mktemp -d)" || return 1
    if ! tar -xzf "$tarball" -C "$dir" 2>/dev/null; then
        rm -rf "$dir"
        echo "ERROR: could not extract the updater payload from $label." >&2
        return 1
    fi
    app="$dir/$top"

    dr="$(codesign -d -r- "$app" 2>&1 || true)"
    dv="$(codesign -dv "$app" 2>&1 || true)"

    if ! codesign --verify --deep --strict "$app" 2>/dev/null; then
        echo "ERROR: the updater payload in $label does not pass codesign --verify --deep --strict." >&2
        rc=1
    fi
    if ! updater_payload_team_id_is_set "$dv"; then
        echo "ERROR: the updater payload in $label carries no Team Identifier, so it is not Developer ID signed." >&2
        rc=1
    fi
    if ! updater_payload_dr_is_developer_id "$dr"; then
        if updater_payload_dr_is_cdhash_pinned "$dr"; then
            echo "ERROR: the updater payload in $label has a cdhash-pinned designated requirement. This is EXACTLY the v0.19.0 bug: the tarball was packed from the app before it was codesigned, so every auto-update would replace a notarized app with an ad-hoc one and destroy the user's macOS permission grants." >&2
        else
            echo "ERROR: the updater payload in $label does not have a Developer ID designated requirement." >&2
        fi
        rc=1
    fi

    if [ "$rc" != "0" ]; then
        echo "       payload:  $top" >&2
        printf '%s\n' "$dv" | grep -E '^(Identifier|Signature|TeamIdentifier|Authority)' | sed 's/^/       /' >&2 || true
        printf '%s\n' "$dr" | grep 'designated' | sed 's/^/       /' >&2 || true
        echo "       Rebuild with APPLE_SIGNING_IDENTITY set so build-dmg.sh signs the bundle and repacks the tarball from it. A staging directory produced before the repack existed must be re-staged, not re-uploaded." >&2
    fi

    rm -rf "$dir"
    return "$rc"
}
