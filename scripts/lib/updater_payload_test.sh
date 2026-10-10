#!/usr/bin/env bash
# Tests for scripts/lib/updater_payload.sh, the repack + re-sign + publish gate
# for the Tauri updater payload (`Lucidos.app.tar.gz`).
# Run: ./scripts/lib/updater_payload_test.sh
#
# Three tiers, so as much as possible runs everywhere:
#   • the pure codesign-output predicates and the archive-shape rules need only
#     `tar`, and run on every host;
#   • the repack round trip needs `codesign` plus the stable dev signing identity
#     (./scripts/dev-codesign-setup.sh), and says so when it skips;
#   • the re-sign needs the tauri CLI, and generates its own throwaway key.
#
# A genuine Developer ID signature needs the release certificate and a 40-minute
# build, so no test here produces one. That is exactly why the gate's decision is
# split into pure text predicates: the ACCEPT direction is covered against real
# captured `codesign` output, and the REJECT direction is covered end to end
# against a genuinely ad-hoc payload (the v0.19.0 shape).
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/updater_payload.sh
source "$SCRIPT_DIR/updater_payload.sh"
# shellcheck source=scripts/lib/codesign.sh
source "$SCRIPT_DIR/codesign.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }
skip() { echo "  skip: $*"; }

WORK="$(mktemp -d)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

# ── 1. Pure predicates over captured codesign output ─────────────────────────
# These are the REAL shapes, with the team identifier and bundle id generalized
# (the predicates key on structure, not on our identifiers). The cdhash is left
# as-is: it is the code-directory hash of a published artifact and names nobody.

DR_DEVELOPER_ID='Executable=/tmp/Lucidos.app/Contents/MacOS/lucidos-app
designated => identifier "com.example.app" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = AB12CD34EF'

# What v0.19.0 actually shipped in Lucidos.app.tar.gz.
DR_ADHOC='Executable=/Applications/Lucidos.app/Contents/MacOS/lucidos-app
# designated => cdhash H"d3974ae45fa91b7a9df11b9b5e52eb988532a7cb"'

# The stable dev identity (scripts/lib/codesign.sh): certificate-anchored, so it
# survives a rebuild, but self-signed, so it has no team and must never satisfy
# the release gate. The leaf hash is per-machine (dev-codesign-setup.sh generates
# the certificate locally), so the fixture carries a placeholder of the right
# shape rather than any particular machine's.
DR_DEV_IDENTITY='Executable=/tmp/Probe.app/Contents/MacOS/probe
designated => identifier "com.example.app" and certificate leaf = H"0123456789abcdef0123456789abcdef01234567"'

echo "test: designated-requirement predicate"
if updater_payload_dr_is_developer_id "$DR_DEVELOPER_ID"; then
    pass "a Developer ID requirement is accepted"
else
    fail "the real Developer ID requirement was rejected"
fi
if updater_payload_dr_is_developer_id "$DR_ADHOC"; then
    fail "the v0.19.0 cdhash-pinned requirement was accepted"
else
    pass "the v0.19.0 cdhash-pinned requirement is rejected"
fi
if updater_payload_dr_is_developer_id "$DR_DEV_IDENTITY"; then
    fail "the self-signed dev identity satisfied the Developer ID gate"
else
    pass "the self-signed dev identity does not satisfy the Developer ID gate"
fi
# Neither half of the requirement may stand alone.
if updater_payload_dr_is_developer_id 'designated => anchor apple generic'; then
    fail "an anchor with no team was accepted"
else
    pass "an anchor with no team is rejected"
fi
if updater_payload_dr_is_developer_id 'designated => certificate leaf[subject.OU] = AB12CD34EF'; then
    fail "a team term with no anchor was accepted"
else
    pass "a team term with no anchor is rejected"
fi

echo ""
echo "test: cdhash-pinned predicate tells the v0.19.0 case from other failures"
if updater_payload_dr_is_cdhash_pinned "$DR_ADHOC"; then
    pass "the ad-hoc requirement is identified as cdhash-pinned"
else
    fail "the ad-hoc requirement was not identified as cdhash-pinned"
fi
for label_dr in "developer-id:$DR_DEVELOPER_ID" "dev-identity:$DR_DEV_IDENTITY"; do
    if updater_payload_dr_is_cdhash_pinned "${label_dr#*:}"; then
        fail "${label_dr%%:*} was misreported as cdhash-pinned"
    else
        pass "${label_dr%%:*} is not reported as cdhash-pinned"
    fi
done

echo ""
echo "test: TeamIdentifier predicate"
if updater_payload_team_id_is_set 'Identifier=com.example.app
Signature=Developer ID Application
TeamIdentifier=AB12CD34EF'; then
    pass "a real team identifier is accepted"
else
    fail "a real team identifier was rejected"
fi
if updater_payload_team_id_is_set 'Identifier=lucidos_app-e0ed54f1c3141357
Signature=adhoc
TeamIdentifier=not set'; then
    fail "'TeamIdentifier=not set' was accepted (the v0.19.0 payload)"
else
    pass "'TeamIdentifier=not set' is rejected (the v0.19.0 payload)"
fi
if updater_payload_team_id_is_set 'Identifier=com.example.app'; then
    fail "a missing TeamIdentifier line was accepted"
else
    pass "a missing TeamIdentifier line is rejected"
fi

# ── 2. Archive shape (tar only) ──────────────────────────────────────────────
# The three rules tauri-plugin-updater's `skip(1)` + `Entry::unpack` impose.

make_tarball() {  # <name> <builder-fn>
    local name="$1" builder="$2" root="$WORK/src-$1"
    mkdir -p "$root"
    ( cd "$root" && "$builder" )
    # Glob rather than `.`, so entries are `Lucidos.app/…` and not `./Lucidos.app/…`
    # (which would make every archive look like it had one top-level entry named
    # `.` and defeat the very rule under test). Every builder creates at least one
    # entry, so the glob always matches.
    ( cd "$root" && COPYFILE_DISABLE=1 tar -czf "$WORK/$name.tar.gz" -- * )
    printf '%s' "$WORK/$name.tar.gz"
}

build_good()      { mkdir -p Lucidos.app/Contents/MacOS; printf 'x\n' > Lucidos.app/Contents/Info.plist; }
build_two_roots() { mkdir -p Lucidos.app/Contents; printf 'x\n' > Lucidos.app/Contents/Info.plist; printf 'y\n' > stray.txt; }
build_no_app()    { mkdir -p Contents/MacOS; printf 'x\n' > Contents/Info.plist; }
build_hardlink()  { mkdir -p Lucidos.app/Contents; printf 'x\n' > Lucidos.app/Contents/one; ln Lucidos.app/Contents/one Lucidos.app/Contents/two; }

# The AppleDouble fixture is written directly rather than packed from a
# directory: this libarchive silently DROPS a literal `._foo` file while walking
# a tree, so there is no way to make it emit the entry under test. Building the
# archive is also the more honest fixture, since the rule is a property of the
# ARCHIVE, and the staging gate inspects archives it did not create.
make_appledouble_tarball() {
    OUT="$WORK/appledouble.tar.gz" python3 - <<'PY'
import io, os, tarfile

def add(tf, name, kind, data=b""):
    info = tarfile.TarInfo(name)
    info.type = kind
    info.mode = 0o755 if kind == tarfile.DIRTYPE else 0o644
    info.size = len(data)
    tf.addfile(info, io.BytesIO(data) if data else None)

with tarfile.open(os.environ["OUT"], "w:gz") as tf:
    add(tf, "Lucidos.app", tarfile.DIRTYPE)
    add(tf, "Lucidos.app/Contents", tarfile.DIRTYPE)
    add(tf, "Lucidos.app/Contents/Info.plist", tarfile.REGTYPE, b"x\n")
    add(tf, "Lucidos.app/Contents/._Info.plist", tarfile.REGTYPE, b"z\n")
PY
    printf '%s' "$WORK/appledouble.tar.gz"
}

echo ""
echo "test: archive shape accepts the layout the updater unpacks"
T="$(make_tarball good build_good)"
if top="$(updater_payload_assert_archive_shape "$T" Lucidos.app 2>&1)" && [ "$top" = "Lucidos.app" ]; then
    pass "a single .app top-level component is accepted and reported"
else
    fail "the good layout was rejected: $top"
fi
if updater_payload_assert_archive_shape "$T" Other.app >/dev/null 2>&1; then
    fail "a top-level component that disagrees with the signed bundle was accepted"
else
    pass "a top-level component that disagrees with the signed bundle is refused"
fi

echo ""
echo "test: archive shape refuses what the updater cannot unpack"
T="$(make_tarball two-roots build_two_roots)"
out="$(updater_payload_assert_archive_shape "$T" 2>&1)"; rc=$?
if [ "$rc" -ne 0 ] && echo "$out" | grep -qi "top-level"; then
    pass "two top-level entries are refused (skip(1) would strip only one)"
else
    fail "two top-level entries were accepted: $out"
fi

T="$(make_tarball no-app build_no_app)"
out="$(updater_payload_assert_archive_shape "$T" 2>&1)"; rc=$?
if [ "$rc" -ne 0 ] && echo "$out" | grep -qi "not a .app"; then
    pass "an archive rooted at Contents/ is refused"
else
    fail "an archive rooted at Contents/ was accepted: $out"
fi

T="$(make_tarball hardlink build_hardlink)"
if tar -tvzf "$T" | grep -q '^h'; then
    out="$(updater_payload_assert_archive_shape "$T" 2>&1)"; rc=$?
    if [ "$rc" -ne 0 ] && echo "$out" | grep -qi "hard-link"; then
        pass "hard-link entries are refused (Entry::unpack resolves them against the CWD)"
    else
        fail "hard-link entries were accepted: $out"
    fi
else
    skip "this tar did not emit a hard-link entry; nothing to assert"
fi

T="$(make_appledouble_tarball)"
out="$(updater_payload_assert_archive_shape "$T" 2>&1)"; rc=$?
if [ "$rc" -ne 0 ] && echo "$out" | grep -qi "AppleDouble"; then
    pass "AppleDouble (._) entries are refused (they break the CodeResources seal)"
else
    fail "AppleDouble entries were accepted: $out"
fi

# ── 3. A real bundle: repack round trip + the publish gate ───────────────────
# Builds a miniature .app with real Mach-O files, a nested Resources tree and a
# symlink, so the round trip exercises what the real bundle contains.

make_app() {  # <dir>
    local app="$1"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/postgres/lib" \
             "$app/Contents/Resources/frontend"
    cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>probe</string>
<key>CFBundleIdentifier</key><string>com.example.updaterpayloadprobe</string>
<key>CFBundleName</key><string>Probe</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>1.0</string>
<key>CFBundleVersion</key><string>1.0</string>
</dict></plist>
PLIST
    cp /bin/echo "$app/Contents/MacOS/probe"
    # A loose Mach-O under Resources, standing in for the ~200 relocatable
    # Postgres binaries that `codesign --deep` alone never signs.
    cp /bin/cat "$app/Contents/Resources/postgres/lib/loose-macho"
    printf 'hello\n' > "$app/Contents/Resources/frontend/index.html"
    ln -s index.html "$app/Contents/Resources/frontend/alias.html"
}

# Sign inside-out, the same walk sign_app_bundle does: every Mach-O deepest-first,
# then the outer bundle. A plain --deep would leave the loose Resources binary
# unsigned and the outer seal would still verify, which is the trap.
sign_app_inside_out() {  # <app> <identity> [codesign-arg…]
    local app="$1" identity="$2"; shift 2
    local path
    while IFS= read -r -d '' path; do
        case "$(file -b "$path")" in
            *Mach-O*) codesign --force "$@" --sign "$identity" "$path" >/dev/null 2>&1 || return 1 ;;
        esac
    done < <(/usr/bin/find "$app" -type f -print0 | sort -rz)
    codesign --force --deep "$@" --sign "$identity" "$app" >/dev/null 2>&1
}

HAVE_CODESIGN=0
command -v codesign >/dev/null 2>&1 && [ "$(uname -s)" = "Darwin" ] && HAVE_CODESIGN=1

echo ""
echo "test: the publish gate refuses an ad-hoc payload (the v0.19.0 case)"
if [ "$HAVE_CODESIGN" != "1" ]; then
    skip "no codesign on this host"
else
    ADHOC_APP="$WORK/adhoc/Lucidos.app"
    make_app "$ADHOC_APP"
    if sign_app_inside_out "$ADHOC_APP" - ; then
        ( cd "$WORK/adhoc" && COPYFILE_DISABLE=1 tar -czf "$WORK/adhoc.tar.gz" Lucidos.app )
        out="$(updater_payload_assert_developer_id "$WORK/adhoc.tar.gz" "staging/adhoc" 2>&1)"; rc=$?
        if [ "$rc" -ne 0 ]; then
            pass "an ad-hoc payload is refused"
        else
            fail "an ad-hoc payload passed the publish gate"
        fi
        case "$out" in
            *"v0.19.0"*) pass "the refusal names the incident it exists to prevent" ;;
            *) fail "the refusal does not name the v0.19.0 case: $out" ;;
        esac
        case "$out" in
            *"re-staged"*) pass "the refusal says a pre-repack staging must be re-staged" ;;
            *) fail "the refusal does not say how to recover: $out" ;;
        esac
    else
        fail "could not ad-hoc sign the probe bundle"
    fi
fi

echo ""
echo "test: repack round trip preserves the code signature"
if [ "$HAVE_CODESIGN" != "1" ]; then
    skip "no codesign on this host"
elif ! lucidos_signing_identity_ready; then
    skip "the dev signing identity is not set up (./scripts/dev-codesign-setup.sh)"
else
    lucidos_ensure_keychain_in_search_list
    security unlock-keychain -p "$LUCIDOS_SIGNING_KC_PASS" "$LUCIDOS_SIGNING_KEYCHAIN" 2>/dev/null || true
    SIGNED_APP="$WORK/signed/Lucidos.app"
    make_app "$SIGNED_APP"
    # --timestamp=none for the same reason build-dmg.sh's dev path passes it: a
    # secure timestamp means a network round trip per file for a certificate no
    # one else trusts.
    if sign_app_inside_out "$SIGNED_APP" "$LUCIDOS_SIGNING_IDENTITY" \
            --timestamp=none --keychain "$LUCIDOS_SIGNING_KEYCHAIN"; then
        pass "signed the probe bundle inside-out with the dev identity"

        # A STALE tarball, standing in for the one cargo tauri build leaves
        # behind: packed before the bundle was signed.
        STALE="$WORK/stale/Lucidos.app"
        make_app "$STALE"
        ( cd "$WORK/stale" && COPYFILE_DISABLE=1 tar -czf "$WORK/payload.tar.gz" Lucidos.app )
        STALE_SUM="$(shasum -a 256 "$WORK/payload.tar.gz" | cut -d' ' -f1)"

        if updater_payload_repack "$SIGNED_APP" "$WORK/payload.tar.gz" >/dev/null 2>&1; then
            pass "repack succeeded"
        else
            fail "repack failed for a correctly signed bundle"
        fi
        if [ "$(shasum -a 256 "$WORK/payload.tar.gz" | cut -d' ' -f1)" != "$STALE_SUM" ]; then
            pass "the stale tarball was actually replaced"
        else
            fail "the tarball is unchanged after a repack"
        fi
        if [ -e "$WORK/payload.tar.gz.repack.tmp" ]; then
            fail "the repack left its temp archive behind"
        else
            pass "no temp archive is left next to the payload"
        fi

        # The round trip the updater will do.
        mkdir -p "$WORK/roundtrip"
        tar -xzf "$WORK/payload.tar.gz" -C "$WORK/roundtrip"
        if codesign --verify --deep --strict "$WORK/roundtrip/Lucidos.app" 2>/dev/null; then
            pass "the extracted payload passes codesign --verify --deep --strict"
        else
            fail "the extracted payload fails codesign --verify --deep --strict"
        fi
        if [ -L "$WORK/roundtrip/Lucidos.app/Contents/Resources/frontend/alias.html" ]; then
            pass "a symlink survives the round trip as a symlink"
        else
            fail "a symlink did not survive the round trip"
        fi
        if [ -x "$WORK/roundtrip/Lucidos.app/Contents/MacOS/probe" ]; then
            pass "the executable bit survives the round trip"
        else
            fail "the executable bit did not survive the round trip"
        fi
        # The loose Resources Mach-O must still carry its own signature: this is
        # what a plain --deep would have missed, and what notarization rejects.
        if codesign --verify --strict \
                "$WORK/roundtrip/Lucidos.app/Contents/Resources/postgres/lib/loose-macho" 2>/dev/null; then
            pass "a loose Mach-O under Resources keeps its signature"
        else
            fail "a loose Mach-O under Resources lost its signature"
        fi

        # The dev identity is certificate-anchored (so TCC grants survive a
        # rebuild) but self-signed, so it must NOT satisfy the release gate.
        if updater_payload_assert_developer_id "$WORK/payload.tar.gz" "staging/dev" >/dev/null 2>&1; then
            fail "a dev-identity payload satisfied the Developer ID publish gate"
        else
            pass "a dev-identity payload does not satisfy the Developer ID publish gate"
        fi

        # THE HARD GATE. A bundle whose seal is broken must be refused at repack
        # time, not discovered by a user whose app will not launch.
        BROKEN="$WORK/broken/Lucidos.app"
        make_app "$BROKEN"
        sign_app_inside_out "$BROKEN" "$LUCIDOS_SIGNING_IDENTITY" \
            --timestamp=none --keychain "$LUCIDOS_SIGNING_KEYCHAIN"
        printf 'unsealed\n' > "$BROKEN/Contents/Resources/frontend/smuggled.html"
        ( cd "$WORK/broken" && COPYFILE_DISABLE=1 tar -czf "$WORK/broken.tar.gz" Lucidos.app )
        out="$(updater_payload_repack "$BROKEN" "$WORK/broken.tar.gz" 2>&1)"; rc=$?
        if [ "$rc" -ne 0 ] && echo "$out" | grep -qi "round trip"; then
            pass "a bundle whose seal is broken is refused at repack time"
        else
            fail "a broken seal survived the repack gate: $out"
        fi
        if [ -e "$WORK/broken.tar.gz.repack.tmp" ]; then
            fail "the refused repack left its temp archive behind"
        else
            pass "a refused repack leaves no temp archive"
        fi
    else
        fail "could not sign the probe bundle with the dev identity"
    fi
fi

# ── 4. Re-signing the repacked bytes ─────────────────────────────────────────
echo ""
echo "test: the .sig is regenerated over the repacked bytes"
HAVE_TAURI=0
KEY="$WORK/updater.key"
if command -v cargo >/dev/null 2>&1 && cargo tauri --version >/dev/null 2>&1; then
    if cargo tauri signer generate --ci --password "" -w "$KEY" >/dev/null 2>&1 && [ -f "$KEY" ]; then
        HAVE_TAURI=1
    fi
fi
if [ "$HAVE_TAURI" != "1" ]; then
    skip "the tauri CLI is not available to generate a throwaway updater key"
else
    PAYLOAD="$WORK/resign.tar.gz"
    printf 'pretend pre-repack payload\n' > "$PAYLOAD"
    # The throwaway key is pointed at through the environment, in a subshell, so
    # a release-capable shell's REAL updater key (every Lucidos-spawned process
    # inherits one) cannot be the thing under test. The locality is the design,
    # which is what SC2030/SC2031 are reporting.
    # shellcheck disable=SC2030,SC2031
    use_throwaway_key() {
        export TAURI_SIGNING_PRIVATE_KEY_PATH="$KEY"
        export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
        unset TAURI_SIGNING_PRIVATE_KEY
    }
    ( use_throwaway_key; tauri_signer_sign_file "$PAYLOAD" )
    if [ -s "$PAYLOAD.sig" ]; then
        pass "the shared signer produces a .sig"
        BEFORE="$(cat "$PAYLOAD.sig")"
        printf 'pretend REPACKED payload\n' > "$PAYLOAD"
        if ( use_throwaway_key; updater_payload_resign "$PAYLOAD" ); then
            pass "updater_payload_resign succeeds over the repacked bytes"
        else
            fail "updater_payload_resign failed with a valid key"
        fi
        if [ "$(cat "$PAYLOAD.sig")" != "$BEFORE" ]; then
            pass "the signature changed, so the stale one cannot ship"
        else
            fail "the signature is unchanged after re-signing different bytes"
        fi
    else
        fail "the shared signer produced no .sig with a freshly generated key"
    fi

    # THE BACK-COMPAT BRANCH, on its success path. TAURI_SIGNING_PRIVATE_KEY may
    # hold the key CONTENTS rather than a path, and that is the branch carrying
    # actual secret material, so a miswiring there is the expensive one. Every
    # other test reaches it only when it is expected to FAIL (a non-signing key),
    # which would pass just as well if the variable name were wrong.
    CONTENTS_PAYLOAD="$WORK/contents.tar.gz"
    KEY_CONTENTS="$(cat "$KEY")"
    printf 'payload signed from key contents\n' > "$CONTENTS_PAYLOAD"
    if (
        unset TAURI_SIGNING_PRIVATE_KEY_PATH
        export TAURI_SIGNING_PRIVATE_KEY="$KEY_CONTENTS"
        export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
        tauri_signer_sign_file "$CONTENTS_PAYLOAD"
    ) && [ -s "$CONTENTS_PAYLOAD.sig" ]; then
        pass "the key CONTENTS form (legacy TAURI_SIGNING_PRIVATE_KEY) signs too"
    else
        fail "signing with the key contents in TAURI_SIGNING_PRIVATE_KEY produced no .sig"
    fi

    # No key configured at all must fail loud rather than leaving the stale .sig
    # sitting next to repacked bytes, which every updater would then reject.
    if env -u TAURI_SIGNING_PRIVATE_KEY -u TAURI_SIGNING_PRIVATE_KEY_PATH \
            bash -c "source '$SCRIPT_DIR/updater_payload.sh'; updater_payload_resign '$PAYLOAD'" \
            >/dev/null 2>&1; then
        fail "re-signing without a key reported success"
    else
        pass "re-signing without a key fails loud"
    fi
fi

# ── 5. One signer call site ──────────────────────────────────────────────────
echo ""
echo "test: the updater signer is invoked from one file only"
# Two callers need it (the release preflight's throwaway test-sign and the
# repack's re-sign) and they must not drift: a second copy of the key-resolution
# rules is how a release ends up with a key that is set but emits no .sig. The
# invariant is one OWNING file, not one line: tauri_signer_sign_file spells the
# invocation out per key form, because a prefix assignment cannot take a dynamic
# variable name without an `eval` holding key material.
#
# Comment lines are dropped first: three files name the command in prose
# precisely to record that rule. This file is excluded outright, since the lines
# above name it too.
SIGN_FILES="$(cd "$PROJECT_DIR" \
    && git grep -n 'cargo tauri signer sign' -- scripts ':(exclude)scripts/lib/updater_payload_test.sh' \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*#' \
    | cut -d: -f1 | sort -u || true)"
if [ "$SIGN_FILES" = "scripts/lib/tauri_signing_key.sh" ]; then
    pass "the only file invoking the signer is scripts/lib/tauri_signing_key.sh"
else
    fail "expected tauri_signing_key.sh to be the only signer call site, found: $SIGN_FILES"
fi

echo ""
echo "test: the updater key never reaches the signer's argv"
# `ps -eo command` is world-readable. The legacy TAURI_SIGNING_PRIVATE_KEY form
# holds the key CONTENTS, so a `--private-key <material>` flag would publish the
# updater private key to every local process while the sign runs, and a
# passworded key would publish its password too. Same rule notarytool_run follows
# for APPLE_PASSWORD, and it binds harder here because this lib SHIPS to the
# public mirror. The key travels through exported vars in the invocation's own
# subshell instead (`env VAR=v cmd` would not do: that lands in env's argv).
#
# Check the CALL SITES, not the file: the header comment names the flags
# deliberately, to record why they may not come back. Drop comment lines, fold
# backslash continuations onto one line, then keep the signer invocations, so
# each one is inspected together with the prefix assignments in front of it.
SIGNER_CALLS="$(grep -vE '^[[:space:]]*#' "$SCRIPT_DIR/tauri_signing_key.sh" \
    | sed -e ':a' -e '/\\$/{N;s/\\\n//;ba' -e '}' \
    | grep 'cargo tauri signer' || true)"
if [ -n "$SIGNER_CALLS" ]; then
    pass "the signer is still invoked from tauri_signing_key.sh"
else
    fail "found no signer invocation in tauri_signing_key.sh"
fi
if printf '%s\n' "$SIGNER_CALLS" \
        | grep -qE -- '--private-key|--password|(^|[[:space:]])-[kfp]([[:space:]]|$)'; then
    fail "the signer call site passes the key or password on the command line: $SIGNER_CALLS"
else
    pass "no key material or password in the signer's argv"
fi
# And the values must actually reach it, or the calls above sign with nothing.
# Every invocation must carry the password and one key var as PREFIX assignments
# (bash keeps those out of argv) and clear the other key var with `env -u`, so
# which key is used is decided here rather than inherited.
MISWIRED=0
while IFS= read -r call; do
    [ -n "$call" ] || continue
    # shellcheck disable=SC2016 # matching the literal source text, not expanding it
    case "$call" in
        *'TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$pw"'*) ;;
        *) MISWIRED=1 ;;
    esac
    # shellcheck disable=SC2016 # matching the literal source text, not expanding it
    case "$call" in
        *'TAURI_SIGNING_PRIVATE_KEY_PATH="$key"'*|*'TAURI_SIGNING_PRIVATE_KEY="$key"'*) ;;
        *) MISWIRED=1 ;;
    esac
    case "$call" in
        *'env -u TAURI_SIGNING_PRIVATE_KEY'*) ;;
        *) MISWIRED=1 ;;
    esac
done <<< "$SIGNER_CALLS"
if [ "$MISWIRED" = "0" ]; then
    pass "every invocation passes the key + password as prefix assignments and clears the other key var"
else
    fail "a signer invocation does not receive the key through prefix assignments: $SIGNER_CALLS"
fi

echo ""
echo "updater_payload: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
