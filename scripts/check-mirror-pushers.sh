#!/usr/bin/env bash
# check-mirror-pushers.sh: fail if anything but release_tree_push can write to
# the public mirror (ADR 0462). /harden Phase 4.5 runs it on every change.
#
#   ./scripts/check-mirror-pushers.sh [<repo-root>]
#
# Whole-tree, over tracked shell scripts, the Makefile, workflows and Rust.
# Two arms, each with a short allowlist:
#
#   1. A git push. Shell, Makefile and workflow lines match `git [-C <dir>]
#      push`, and Rust matches a "push" git argument. Comment lines are skipped.
#      Allowed:
#        scripts/lib/release_tree.sh       the one gated pusher
#        scripts/lib/release_main_sync.sh  pushes origin, never the mirror
#        scripts/release.sh                only lines naming $ORIGIN_REMOTE
#        a Rust line naming "origin"       the engine's main push, and fixtures
#        scripts/lib/*_test.sh             throwaway repos only
#   2. The mirror's git URL. Allowed in release_tree.sh, in a test, and on
#      install.sh's LUCIDOS_REPO_URL line, which only clones.
#
# Fix a failure by routing the push through release_tree_push. Never widen the
# allowlist for a script that writes to the mirror.
#
# Exit 0 clean, 1 on a hit or when the scan cannot run.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${1:-$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null)}"
if [ -z "$ROOT" ] || ! git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    echo "ERROR: not inside a git checkout, so the mirror-pusher scan cannot run." >&2
    exit 1
fi

SHELL_PUSH='(^|[^[:alnum:]_./-])git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:]]+|[[:space:]]+--(git-dir|work-tree)=[^[:space:]]+)*[[:space:]]+push([[:space:]]|$|["'"'"');|&])'
RUST_PUSH='\[[[:space:]]*"push"[[:space:]]*[],]|\.arg\([[:space:]]*"push"[[:space:]]*\)'
MIRROR_URL='lucidos-dev/lucidos\.git|git@github\.com:lucidos-dev/lucidos'

# scan <pattern> <pathspec...>: print path:line:text for every tracked match.
# git grep exits 1 on no match, which is clean; anything above 1 is a failure.
scan() {
    local pattern="$1" rc
    shift
    git -C "$ROOT" grep -n -I -E -e "$pattern" -- "$@"
    rc=$?
    [ "$rc" -le 1 ] || { echo "ERROR: git grep failed (status $rc)" >&2; return 2; }
    return 0
}

is_comment() { # <text>
    [[ "$1" =~ ^[[:space:]]*(#|//) ]]
}

# shellcheck disable=SC2016 # $ORIGIN_REMOTE is matched literally, never expanded
push_is_allowed() { # <path> <text>
    case "$1" in
        scripts/lib/release_tree.sh | scripts/lib/release_main_sync.sh | scripts/lib/*_test.sh) return 0 ;;
        scripts/release.sh) [[ "$2" == *'$ORIGIN_REMOTE'* ]] ;;
        *.rs) [[ "$2" == *'"origin"'* ]] ;;
        *) return 1 ;;
    esac
}

url_is_allowed() { # <path> <text>
    case "$1" in
        scripts/lib/release_tree.sh | scripts/lib/*_test.sh) return 0 ;;
        install.sh) [[ "$2" =~ ^LUCIDOS_REPO_URL= ]] ;;
        *) return 1 ;;
    esac
}

hits=""
record() { # <arm> <path> <line> <text>
    hits="$hits
  $1  $2:$3: $4"
}

# check_lines <arm> <skip-comments> <allow-fn>: read path:line:text on stdin.
check_lines() {
    local arm="$1" skip_comments="$2" allow="$3" path line text
    while IFS=: read -r path line text; do
        [ -n "$path" ] || continue
        if [ "$skip_comments" = 1 ] && is_comment "$text"; then
            continue
        fi
        if ! "$allow" "$path" "$text"; then
            record "$arm" "$path" "$line" "$text"
        fi
    done
}

SHELL_SPEC=('*.sh' 'Makefile' '*/Makefile' '*.yml' '*.yaml')
out_shell="$(scan "$SHELL_PUSH" "${SHELL_SPEC[@]}")" || exit 1
out_rust="$(scan "$RUST_PUSH" '*.rs')" || exit 1
out_url="$(scan "$MIRROR_URL" "${SHELL_SPEC[@]}" '*.rs')" || exit 1

check_lines push 1 push_is_allowed <<<"$out_shell"
check_lines push 1 push_is_allowed <<<"$out_rust"
check_lines url 0 url_is_allowed <<<"$out_url"

if [ -n "$hits" ]; then
    echo "✗ BLOCKED: something other than release_tree_push can write to the public mirror:"
    printf '%s\n' "$hits"
    echo ""
    echo "  Push to the mirror through release_tree_push (scripts/lib/release_tree.sh),"
    echo "  which scans the tree and checks the ancestry first. See ADR 0462."
    exit 1
fi
echo "✓ only release_tree_push writes to the public mirror"
