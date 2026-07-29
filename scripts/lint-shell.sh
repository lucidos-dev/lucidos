#!/usr/bin/env bash
#
# lint-shell.sh — run ShellCheck over every shell script in the repo.
#
#   ./scripts/lint-shell.sh          # or: make lint-shell  (also part of `make lint` / `make check`)
#
# Discovery is `git ls-files '*.sh'`, NOT a hand-maintained path list. Every
# shell script in this repo carries a .sh extension (there are no extensionless
# ones), so that glob is exact — and self-maintaining: a script added anywhere,
# in any directory, is linted the day it is committed. A hand-written list is
# the failure mode `.claude/rules/` already documents, where a rule silently
# fails to cover a new file and looks exactly like a rule that does not exist.
#
# Flags live in the tracked .shellcheckrc at the repo root, deliberately NOT in
# this script's invocation: a bare `shellcheck scripts/foo.sh` typed by hand, an
# editor's inline ShellCheck, and this gate must all reach the same verdict. A
# gate that disagrees with the tool a developer runs by hand trains people to
# distrust the gate.
#
# Exit status: 0 when every script is clean, non-zero otherwise — including when
# ShellCheck is missing or discovery finds nothing. A gate that cannot run must
# never read as "clean".

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR" || exit 1

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_GREEN="$(printf '\033[32m')"; C_RED="$(printf '\033[31m')"; C_RESET="$(printf '\033[0m')"
else
    C_GREEN=""; C_RED=""; C_RESET=""
fi

if ! command -v shellcheck >/dev/null 2>&1; then
    cat >&2 <<'EOF'
ERROR: shellcheck is not installed, so the shell lint gate cannot run.

This is a failure, not a skip — a gate that silently passes when its checker is
missing is worse than no gate at all.

Install it:
  macOS          brew install shellcheck
  Debian/Ubuntu  sudo apt-get install -y shellcheck
  Fedora         sudo dnf install -y ShellCheck
  Arch           sudo pacman -S shellcheck
  other          https://github.com/koalaman/shellcheck#installing
EOF
    exit 1
fi

# Fail closed on a broken discovery: an empty list would otherwise sail through
# as "everything is clean".
#
# Read into the array with a loop rather than `mapfile`: macOS ships bash 3.2,
# which has neither mapfile nor a `${#arr[@]}` that survives `set -u` on an
# EMPTY array. Counting as we go sidesteps both, and keeps the expansion below
# safe because it only runs once COUNT proves the array is non-empty.
FILES=()
COUNT=0
while IFS= read -r f; do
    [ -n "$f" ] || continue
    FILES+=("$f")
    COUNT=$((COUNT + 1))
done < <(git ls-files '*.sh' 2>/dev/null)

if [ "$COUNT" -eq 0 ]; then
    echo "ERROR: found no tracked *.sh files to lint (is $PROJECT_DIR a git checkout?)." >&2
    exit 1
fi

if [ ! -f .shellcheckrc ]; then
    echo "ERROR: .shellcheckrc is missing from $PROJECT_DIR." >&2
    echo "       It configures source resolution; without it this gate reports findings" >&2
    echo "       that are really just unresolved \`source\` lines." >&2
    exit 1
fi

echo "Linting $COUNT shell scripts with $(shellcheck --version | awk '/^version:/ { print "shellcheck " $2 }')..."

if shellcheck "${FILES[@]}"; then
    printf '%s✓%s shellcheck: %d files, no findings\n' "$C_GREEN" "$C_RESET" "$COUNT"
    exit 0
fi

printf '\n%s✗%s shellcheck reported findings (see above).\n' "$C_RED" "$C_RESET" >&2
cat >&2 <<'EOF'

Fix them at the source. Suppress ONLY a genuine false positive, and always with
a reason on the same line naming why:

  # shellcheck disable=SC2034 # read by scripts/web-dev.sh after it calls this parser

For an unresolvable `source`, prefer a directive over a disable — it keeps the
sourced file analysed:

  # shellcheck source=scripts/lib/service.sh
EOF
exit 1
