#!/usr/bin/env bash
# release_dmg.sh: which file under target/release/bundle/dmg is THE release DMG.
#
# dmg_volume_assemble writes two intermediates beside the final artifact:
#   Lucidos_<ver>_aarch64.rw.dmg     read-write volume
#   Lucidos_<ver>_aarch64.zlib.dmg   compressed output before atomic rename
# A killed run can leave either behind. Discovery must exclude both (F4).
# This library owns their names so assembly and discovery cannot drift.
# It ships with build-dmg.sh on the public mirror.
# Tests: scripts/lib/release_dmg_test.sh.

# release_dmg_rw_path <dmg>: the read-write assembly intermediate.
release_dmg_rw_path() {
    printf '%s' "${1%.dmg}.rw.dmg"
}

# release_dmg_zlib_path <dmg>: the recompressed intermediate dmg_volume_assemble
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
            echo "       Only DMG assembly intermediates are present, and those are never the release DMG:" >&2
            printf '%s\n' "$listing" | sed 's/^/         /' >&2
            echo "       A run killed mid-assembly leaves these behind. Delete them and rebuild." >&2
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
