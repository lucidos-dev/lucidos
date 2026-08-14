#!/usr/bin/env bash
# Run a heavy build under a *build slot*, degrading to a plain run when the
# broker is not there.
#
# This script RESOLVES the broker; it is not the broker. All slot semantics
# (the pool, the count, waiting, announcing) live in the `lucidos` CLI and the
# `lucidos-build-slot` crate. Shell-side locking was ruled out because macOS
# ships no `flock` binary, and that objection does not apply to a Rust binary
# taking an `fs2` flock. See docs/adr/0070-engine-owned-build-slot.md.
#
# Usage:
#   scripts/with-build-slot.sh [--label "<text>"] -- <command> [args...]
#
# It FAILS OPEN, always. A plain `git clone` has no `lucidos` binary, and
# `make lint` must still work there, so a missing broker means the command runs
# unrestricted with one line on stderr. A limiter must never be the reason a
# build cannot happen.
#
# Resolution order for the broker:
#   1. `lucidos` on PATH. The engine puts it there for every spawned session.
#   2. The newest `lucidos` under this checkout's `.launch/`.
#   3. The newest under the MAIN checkout's `.launch/`, found through
#      `git rev-parse --git-common-dir`. A worktree carries a full copy of
#      `scripts/`, so step 2 resolves inside the worktree, where `.launch/`
#      never exists.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

LABEL=""
if [ "${1:-}" = "--label" ]; then
    LABEL="${2:-}"
    shift 2
fi
if [ "${1:-}" = "--" ]; then
    shift
fi

if [ $# -eq 0 ]; then
    echo "with-build-slot.sh: nothing to run" >&2
    echo "usage: scripts/with-build-slot.sh [--label \"<text>\"] -- <command> [args...]" >&2
    exit 2
fi

# The newest executable named `lucidos` under $1/.launch, or nothing.
#
# Newest wins because a checkout holds one launch dir per profile and feature
# variant (ADR 0022), and any of them can broker a slot. A `while read` loop
# rather than `mapfile`, which macOS bash 3.2 does not have.
_newest_launch_cli() {
    local root="$1" newest="" candidate
    [ -d "$root/.launch" ] || return 1
    while IFS= read -r candidate; do
        [ -x "$candidate" ] || continue
        if [ -z "$newest" ] || [ "$candidate" -nt "$newest" ]; then
            newest="$candidate"
        fi
    done < <(find "$root/.launch" -maxdepth 3 -type f -name lucidos 2>/dev/null)
    [ -n "$newest" ] || return 1
    printf '%s' "$newest"
}

# The main checkout a worktree belongs to, or nothing when git cannot say.
_main_checkout() {
    local common
    common="$(git -C "$PROJECT_DIR" rev-parse --git-common-dir 2>/dev/null)" || return 1
    [ -n "$common" ] || return 1
    case "$common" in
        /*) ;;
        *) common="$PROJECT_DIR/$common" ;;
    esac
    (cd "$common/.." 2>/dev/null && pwd) || return 1
}

# Does this `lucidos` know the verb?
#
# Load-bearing, not belt-and-braces. The engine puts the CURRENTLY PUBLISHED
# CLI on PATH for every session it spawns, so between this change landing and
# the next engine build, the `lucidos` in front of us predates `build-slot`.
# Unprobed, every `make lint` in that window died on clap's "unrecognized
# subcommand" instead of linting. Same for anyone on an older install.
_broker_knows_the_verb() {
    "$1" build-slot --help >/dev/null 2>&1
}

_resolve_broker() {
    local candidate main
    candidate="$(command -v lucidos 2>/dev/null)" || candidate=""
    if [ -n "$candidate" ] && _broker_knows_the_verb "$candidate"; then
        printf '%s' "$candidate"
        return 0
    fi
    candidate="$(_newest_launch_cli "$PROJECT_DIR")" || candidate=""
    if [ -n "$candidate" ] && _broker_knows_the_verb "$candidate"; then
        printf '%s' "$candidate"
        return 0
    fi
    main="$(_main_checkout)" || main=""
    if [ -n "$main" ]; then
        candidate="$(_newest_launch_cli "$main")" || candidate=""
        if [ -n "$candidate" ] && _broker_knows_the_verb "$candidate"; then
            printf '%s' "$candidate"
            return 0
        fi
    fi
    return 1
}

if ! BROKER="$(_resolve_broker)"; then
    echo "with-build-slot.sh: no \`lucidos\` that knows \`build-slot\`, running the build unrestricted" >&2
    exec "$@"
fi

if [ -n "$LABEL" ]; then
    exec "$BROKER" build-slot --label "$LABEL" -- "$@"
fi
exec "$BROKER" build-slot -- "$@"
