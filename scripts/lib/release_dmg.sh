#!/usr/bin/env bash
# release_dmg.sh: which file under target/release/bundle/dmg is THE release DMG.
#
# ── WHY THIS EXISTS ──────────────────────────────────────────────────────────
# refresh_dmg_payload rewrites the DMG in place by way of two intermediates that
# it writes NEXT TO the real artifact, and both of them match `*.dmg`:
#
#     Lucidos_<ver>_aarch64.rw.dmg     the uncompressed read-write image
#     Lucidos_<ver>_aarch64.zlib.dmg   the recompressed output, before the mv
#
# A run killed between `hdiutil convert` and the trailing `rm -f` leaves one
# behind permanently. build-dmg.sh's main discovery was a bare
# `find "$BUNDLE_DIR/dmg" -name '*.dmg' | head -1`, which returns DIRECTORY
# order (not newest, not sorted), so a leftover could be picked up as the
# release DMG and then signed, notarized, stapled, staged and published. The
# version-stamp guard cannot catch it: `Lucidos_0.19.0_aarch64.rw.dmg` carries
# `_0.19.0_` exactly like the real one does.
#
# The codebase already knew this. `notarize_adopt_submission` excluded both
# suffixes and required exactly one candidate; the main path had neither. That
# is the finding (F4 in docs/audits/2026-08-02-macos-update-path-audit.md), and
# a second copy of the exclusion would have been the same bug waiting to happen
# again. So the suffixes are defined ONCE here, and the two consumers read them
# from the same place:
#
#   1. refresh_dmg_payload asks for the paths it is about to WRITE
#      (release_dmg_rw_path / release_dmg_zlib_path);
#   2. every discovery asks whether a candidate is one of them
#      (release_dmg_is_intermediate, via release_dmg_find).
#
# Adding a third intermediate therefore means touching one predicate, and the
# writer and the exclusion cannot drift apart.
#
# ── Public-mirror safety ─────────────────────────────────────────────────────
# `build-dmg.sh --release-build` is a legitimate public path and this lib is on
# its critical path, so this file must stay OUT of RELEASE_TREE_EXCLUDE_PATHS
# and is sourced unconditionally, exactly like updater_payload.sh.
#
# Pure path arithmetic plus `find`, so the whole file is offline-testable with
# fixture files. Unit tests: scripts/lib/release_dmg_test.sh.

# release_dmg_rw_path <dmg>: the uncompressed read-write intermediate
# refresh_dmg_payload converts the DMG into before it mounts it.
release_dmg_rw_path() {
    printf '%s' "${1%.dmg}.rw.dmg"
}

# release_dmg_zlib_path <dmg>: the recompressed intermediate refresh_dmg_payload
# writes before atomically moving it onto the real DMG. It exists so a failed
# recompress cannot destroy the only good copy of an expensive build output.
release_dmg_zlib_path() {
    printf '%s' "${1%.dmg}.zlib.dmg"
}

# release_dmg_is_intermediate <path>: zero when <path> is one of the two
# intermediates above rather than a release DMG.
#
# Matched on the BASENAME, so a directory somewhere above it that happens to end
# in `.rw.dmg` cannot make a real artifact look like a leftover.
release_dmg_is_intermediate() {
    case "$(basename -- "${1:-}")" in
        *.rw.dmg|*.zlib.dmg) return 0 ;;
        *)                   return 1 ;;
    esac
}

# release_dmg_find <dir>: print the one real DMG directly inside <dir>.
#
# Non-zero with a reason on stderr when there is no candidate or more than one.
# Refusing an ambiguous directory rather than picking one is the point: `head -1`
# over `find` output is directory order, so "pick the first" is really "pick an
# arbitrary one", and the arbitrary one gets signed and published.
#
# `-maxdepth 1` and `-type f` because the release DMG is a file written directly
# into the bundler's output dir; anything nested is not it.
release_dmg_find() {
    local dir="${1:-}" path listing
    local -a candidates=()

    [ -n "$dir" ] || { echo "ERROR: release_dmg_find needs a directory" >&2; return 1; }
    if [ ! -d "$dir" ]; then
        echo "ERROR: no DMG output directory at $dir" >&2
        return 1
    fi

    while IFS= read -r -d '' path; do
        if release_dmg_is_intermediate "$path"; then
            continue
        fi
        candidates+=("$path")
    done < <(/usr/bin/find "$dir" -maxdepth 1 -type f -name '*.dmg' -print0 2>/dev/null | sort -z)

    # The count is read before anything expands the array: macOS ships bash 3.2,
    # where expanding an EMPTY named array under `set -u` is an unbound-variable
    # error rather than an empty list.
    if [ "${#candidates[@]}" -eq 0 ]; then
        echo "ERROR: no .dmg found directly under $dir" >&2
        listing="$(/usr/bin/find "$dir" -maxdepth 1 -type f -name '*.dmg' 2>/dev/null || true)"
        if [ -n "$listing" ]; then
            echo "       Only refresh_dmg_payload intermediates are present, and those are never the release DMG:" >&2
            printf '%s\n' "$listing" | sed 's/^/         /' >&2
            echo "       A run killed mid-refresh leaves these behind. Delete them and rebuild." >&2
        fi
        return 1
    fi
    if [ "${#candidates[@]}" -ne 1 ]; then
        echo "ERROR: ${#candidates[@]} candidate DMGs directly under $dir; refusing to guess which one is the release:" >&2
        printf '%s\n' "${candidates[@]}" | sed 's/^/       /' >&2
        return 1
    fi

    printf '%s' "${candidates[0]}"
}
