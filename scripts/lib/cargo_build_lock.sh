#!/bin/bash
# Tells whether another cargo is building in a cargo profile directory.
#
# Shared by the prunes that delete files a running build may need
# (stale_objects.sh, stale_incremental.sh). Never fight another cargo (ADR 0294).

# shellcheck source=scripts/lib/flock.sh
source "$(dirname "${BASH_SOURCE[0]}")/flock.sh"

# Succeeds when this host can ask about cargo's lock: through `lsof`, or
# through perl's flock where a minimal Linux image ships no lsof.
cargo_build_lock_probe_available() {
    command -v lsof >/dev/null 2>&1 || command -v perl >/dev/null 2>&1
}

# Prints why a prune must wait, or nothing when no other cargo builds in the
# profile directory $1 (`target/debug`). Cargo holds `.cargo-lock` open and
# flocked for a whole build. It tries the lock before it blocks on it, so the
# fallback's brief shared probe only ever delays it.
_cargo_build_lock_holder() {
    local lock="$1/.cargo-lock" rc=0
    [ -e "$lock" ] || return 0
    if command -v lsof >/dev/null 2>&1; then
        if lsof -t -- "$lock" >/dev/null 2>&1; then
            echo "another cargo is building here"
        fi
    elif command -v perl >/dev/null 2>&1; then
        lock_is_held "$lock" 2>/dev/null || rc=$?
        case "$rc" in
            0) echo "another cargo is building here" ;;
            1) ;;
            *) echo "could not read cargo's build lock" ;;
        esac
    else
        echo "no lsof or perl to tell whether another cargo is building here"
    fi
}

# Takes cargo's lock on profile directory $2 exclusively on descriptor $1,
# without waiting, so no cargo starts there until the caller closes it. A cargo
# that tries meanwhile prints "Blocking waiting for file lock" and waits.
# Returns 1 when another process holds it. With no perl it takes nothing and
# returns 0: the caller's `_cargo_build_lock_holder` check is all it has.
cargo_build_lock_take() { # <fd> <profile-dir>
    command -v perl >/dev/null 2>&1 || return 0
    flock_try "$1" "$2/.cargo-lock" ex
}
