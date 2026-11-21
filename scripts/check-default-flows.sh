#!/usr/bin/env bash
# check-default-flows.sh: fail if a copy of a default coding-agent flow differs
# from its system knowhow (ADR 0505). /harden Phase 4.5 runs it on every change.
#
#   ./scripts/check-default-flows.sh [<repo-root>]
#
# A default flow ships in system-knowhow/coding-agent-flows/<name>.md, as the
# lines from its opening marker line to its closing one. Every other tracked
# file holding an opening marker line must carry that block byte for byte.
# A default flow no copy carries fails too. Lucidos's own copies are required:
#
#   implementation-plan  .claude/skills/implementation-plan/SKILL.md
#   hardening            .claude/commands/harden.md
#
# Fix a failure by copying the knowhow block back, or by changing the default
# in the knowhow and every copy together.
#
# Exit 0 clean, 1 on a mismatch or when the scan cannot run.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${1:-$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null)}"
if [ -z "$ROOT" ] || ! git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    echo "ERROR: not inside a git checkout, so the default-flow check cannot run." >&2
    exit 1
fi

FLOWS_DIR="system-knowhow/coding-agent-flows"
OPEN_LINE='^<!-- default-flow: [a-z0-9-]+ -->$'
REQUIRED=(
    "implementation-plan .claude/skills/implementation-plan/SKILL.md"
    "hardening .claude/commands/harden.md"
)

hits=""
record() { # <path> <reason>
    hits="$hits
  $1: $2"
}

# block <file> <name>: print the marked block, markers included. Exit 2 when
# the block opens twice or never closes, 1 when it is absent.
block() {
    awk -v opener="<!-- default-flow: $2 -->" -v closer="<!-- /default-flow: $2 -->" '
        $0 == opener { opens++; inside = 1 }
        inside { print }
        inside && $0 == closer { inside = 0 }
        END {
            if (opens > 1 || inside) exit 2
            if (opens == 0) exit 1
        }
    ' "$ROOT/$1"
}

# names_in <file>: the flow names its opening marker lines carry.
names_in() {
    grep -E "$OPEN_LINE" "$ROOT/$1" | sed -E 's/^<!-- default-flow: ([a-z0-9-]+) -->$/\1/' | sort -u
}

# Plain paths whatever the user's git config, so every listed copy is read.
plain_git() { git -C "$ROOT" -c color.ui=never -c color.grep=never -c core.quotePath=false "$@"; }

defaults="$(plain_git ls-files -- "$FLOWS_DIR/*.md")" || exit 1
copies="$(plain_git grep -l -E -e "$OPEN_LINE" -- . ":(exclude)$FLOWS_DIR/")"
[ "$?" -le 1 ] || { echo "ERROR: git grep failed" >&2; exit 1; }

checked=0
shipped=""
while IFS= read -r copy; do
    [ -n "$copy" ] || continue
    if [ ! -r "$ROOT/$copy" ]; then
        record "$copy" "is listed by git grep but cannot be read"
        continue
    fi
    for name in $(names_in "$copy"); do
        shipped="$shipped
$name"
        source="$FLOWS_DIR/$name.md"
        if ! printf '%s\n' "$defaults" | grep -qxF "$source"; then
            record "$copy" "names default flow '$name', which $FLOWS_DIR/ does not ship"
            continue
        fi
        want="$(block "$source" "$name")" || {
            record "$source" "carries no single closed block for '$name'"
            continue
        }
        got="$(block "$copy" "$name")" || {
            record "$copy" "opens '$name' twice, or never closes it"
            continue
        }
        if [ "$got" != "$want" ]; then
            record "$copy" "its '$name' block differs from $source"
            continue
        fi
        checked=$((checked + 1))
    done
done <<<"$copies"

while IFS= read -r source; do
    [ -n "$source" ] || continue
    name="$(basename "$source" .md)"
    if ! printf '%s\n' "$shipped" | grep -qxF "$name"; then
        record "$source" "ships default flow '$name', which no copy carries"
    fi
done <<<"$defaults"

for entry in "${REQUIRED[@]}"; do
    name="${entry%% *}"
    path="${entry#* }"
    if [ ! -f "$ROOT/$path" ] || ! names_in "$path" | grep -qxF "$name"; then
        record "$path" "lacks its copy of the '$name' default flow"
    fi
done

if [ -n "$hits" ]; then
    echo "✗ BLOCKED: a copy of a default coding-agent flow drifted from its knowhow:"
    printf '%s\n' "$hits"
    echo ""
    echo "  Copy the block from $FLOWS_DIR/ back verbatim, markers included, or change"
    echo "  the default there and in every copy together. See ADR 0505."
    exit 1
fi
echo "✓ $checked default-flow copies match their knowhow"
