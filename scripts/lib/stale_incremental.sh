#!/bin/bash
# Prunes stale crate dirs from cargo's incremental cache.
#
# rustc keeps one `incremental/<crate>-<hash>/` dir per crate and build
# configuration (profile, test, clippy, features, toolchain, RUSTFLAGS). Cargo
# clears the session dirs inside it and never removes the crate dir of a
# configuration that stopped being built, so a build dir grows without bound.
# Stable cargo has no build-dir gc. Temporary measure: docs/temporary-measures.md.

# shellcheck source=scripts/lib/cargo_build_lock.sh
source "$(dirname "${BASH_SOURCE[0]}")/cargo_build_lock.sh"

STALE_INCREMENTAL_MAX_AGE_MIN=$((7 * 24 * 60))
STALE_INCREMENTAL_PRUNE_EVERY_MIN=$((24 * 60))
STALE_INCREMENTAL_TRASH_NAME=".stale-incremental"
STALE_INCREMENTAL_STAMP_NAME=".stale-incremental-pruned"

# Prints the cargo build dir of checkout $1, the dir that holds `incremental/`.
# Cargo answers, so `build.build-dir`, CARGO_TARGET_DIR and `build.target-dir`
# count. A cargo that cannot answer gets the repo convention.
stale_incremental_build_dir() {
    local checkout="$1" json dir=""
    if json="$(cargo metadata --no-deps --offline --format-version 1 \
        --manifest-path "$checkout/Cargo.toml" 2>/dev/null)"; then
        dir="$(printf '%s' "$json" | sed -n 's/.*"build_directory":"\([^"\\]*\)".*/\1/p')"
        [ -n "$dir" ] ||
            dir="$(printf '%s' "$json" | sed -n 's/.*"target_directory":"\([^"\\]*\)".*/\1/p')"
    fi
    case "$dir" in
        /*) printf '%s\n' "$dir" ;;
        *) printf '%s\n' "${CARGO_TARGET_DIR:-$checkout/target}" ;;
    esac
}

# Prints each `incremental` dir up to three levels under build dir $1:
# `<build>/<profile>/incremental` and `<build>/<triple>/<profile>/incremental`.
_incremental_dirs() {
    find "$1" -maxdepth 3 -type d \
        \( -name deps -o -name build -o -name .fingerprint -o -name "$STALE_INCREMENTAL_TRASH_NAME*" \) -prune \
        -o -type d -name incremental -print 2>/dev/null
}

# Prints the stale crate dirs directly inside incremental dir $1. The name
# pattern admits no space or leading dash, so the output is safe for `read`.
_stale_crate_dirs() {
    find "$1" -mindepth 1 -maxdepth 1 -type d -mmin "+$STALE_INCREMENTAL_MAX_AGE_MIN" 2>/dev/null |
        grep -E '/[A-Za-z0-9_][A-Za-z0-9_-]*-[a-z0-9]+$'
    return 0
}

# Moves each stale crate dir of incremental dir $1 into trash dir $2, and
# prints how many it moved. A move that fails prints a warning to stderr.
# Returns 1, moving nothing, when another cargo builds in the profile dir
# above $1. Listing takes a moment, long enough for one to start, and its
# compile of a stale unit would bump the mtime this read. The moves run under
# cargo's own lock, so no cargo starts between the check and a move. Callers
# run this in a subshell, which releases the lock when it exits.
_move_stale_crate_dirs() {
    local stale dir profile moved=0
    profile="$(dirname "$1")"
    stale="$(_stale_crate_dirs "$1")"
    [ -n "$stale" ] || { echo 0; return 0; }
    [ -z "$(_cargo_build_lock_holder "$profile")" ] || return 1
    cargo_build_lock_take 8 "$profile" 2>/dev/null || return 1
    mkdir -p "$2" 2>/dev/null || { echo "WARNING: could not create $2." >&2; echo 0; return 0; }
    while IFS= read -r dir; do
        if mv "$dir" "$2/$(basename "$dir").$$" 2>/dev/null; then
            moved=$((moved + 1))
        else
            echo "WARNING: could not move $dir aside." >&2
        fi
    done <<< "$stale"
    echo "$moved"
}

# Prints the trash dir this run moves stale dirs into: one per prune process,
# so a run never moves into a dir another run is deleting.
stale_incremental_trash_dir() { # <build-dir>
    printf '%s\n' "$1/$STALE_INCREMENTAL_TRASH_NAME.$$"
}

# Moves each stale crate dir of build dir $1 into its trash dir (same volume,
# so atomic). Deleting is the caller's job, so a 100 GB first prune never
# delays a build. Never fails: a problem prints a warning.
prune_stale_incremental_build_dir() {
    local build="$1" stamp incremental profile moved total=0 deferred=0
    stamp="$build/$STALE_INCREMENTAL_STAMP_NAME"
    [ -d "$build" ] || return 0
    if [ -e "$stamp" ] && [ -n "$(find "$stamp" -mmin "-$STALE_INCREMENTAL_PRUNE_EVERY_MIN" 2>/dev/null)" ]; then
        return 0
    fi
    if ! cargo_build_lock_probe_available; then
        echo "Stale incremental dirs: no lsof or perl to tell whether another cargo is building here; skipped."
        touch "$stamp" 2>/dev/null
        return 0
    fi
    while IFS= read -r incremental; do
        [ -n "$incremental" ] || continue
        profile="$(dirname "$incremental")"
        if [ -n "$(_cargo_build_lock_holder "$profile")" ]; then
            deferred=1
            continue
        fi
        if moved="$(_move_stale_crate_dirs "$incremental" "$(stale_incremental_trash_dir "$build")")"; then
            total=$((total + moved))
        else
            deferred=1
        fi
    done <<< "$(_incremental_dirs "$build")"
    if [ "$deferred" -eq 0 ]; then
        touch "$stamp" 2>/dev/null
    fi
    if [ "$total" -gt 0 ]; then
        echo "Pruned $total stale incremental dirs from $build."
    fi
    return 0
}

# Prints the trash dirs to delete: this run's, and any untouched for a day,
# which an interrupted run left behind. Prints nothing when none exist.
_doomed_stale_incremental_trash() { # <build-dir>
    local own
    own="$(stale_incremental_trash_dir "$1")"
    [ -d "$own" ] && printf '%s\n' "$own"
    find "$1" -mindepth 1 -maxdepth 1 -type d -name "$STALE_INCREMENTAL_TRASH_NAME.*" \
        -mmin "+$STALE_INCREMENTAL_PRUNE_EVERY_MIN" ! -path "$own" 2>/dev/null
    return 0
}

# Prunes the build dir of checkout $1, then deletes the moved dirs, and any
# left by an interrupted run, in the background with no stdio, so the build
# that follows never waits on it. Never fails.
prune_stale_incremental() {
    local build doomed
    build="$(stale_incremental_build_dir "$1")"
    prune_stale_incremental_build_dir "$build"
    doomed="$(_doomed_stale_incremental_trash "$build")"
    if [ -n "$doomed" ]; then
        printf '%s\n' "$doomed" | tr '\n' '\0' | xargs -0 rm -rf -- >/dev/null 2>&1 &
    fi
    return 0
}
