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

# ShellCheck runs in small batches, several at once. `make lint` runs this
# outside the build slot, so the slot's memory gate never sees it, and several
# sessions can run it at once. The job count therefore comes from the host's
# spare memory, in lib/lint_shell_jobs.sh. A further batch then starts only
# while memory still has room, and one batch always runs. LINT_SHELL_JOBS sets
# the count and switches the memory check off.
#
# .shellcheckrc makes ShellCheck parse each file's whole `source` tree, once per
# checked file. So the heaviest single file sets the peak of one process, and
# no batch size lowers it. A small batch spreads the heavy files across the
# jobs, and each process gives its memory back when its batch ends.
#
# Findings do not change with the batch size or the job count, since each
# file's result depends only on its own `source` tree. They print in discovery
# order. Only the "For more information:" footer differs: each failing batch
# prints its own.
SHELLCHECK_BATCH_SIZE=4

# A clean run records a key over every input ShellCheck reads. A later run with
# the same key reports the recorded pass and starts no ShellCheck. The record
# stays in this checkout, so no checkout trusts another's pass.
# LINT_SHELL_NO_CACHE=1 forces a run.
PASS_RECORD=.lucidos/lint-shell-passed
PASS_RECORD_KEEP=20

# How long the dispatcher sleeps before it looks for a free job again.
DISPATCH_POLL_SECS=0.2

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

JOBS="${LINT_SHELL_JOBS:-}"
MEMORY_CHECKED=""
if [ -z "$JOBS" ]; then
    # shellcheck source=scripts/lib/lint_shell_jobs.sh
    source "$SCRIPT_DIR/lib/lint_shell_jobs.sh"
    JOBS="$(lint_shell_jobs)"
    MEMORY_CHECKED=1
fi
case "$JOBS" in
    '' | 0* | *[!0-9]*)
        echo "ERROR: LINT_SHELL_JOBS must be a positive whole number, got '$JOBS'." >&2
        exit 1
        ;;
esac

WORK="$(mktemp -d)" || exit 1
trap 'rm -rf "$WORK"' EXIT

# Fail closed on a broken discovery: an empty list would otherwise sail through
# as "everything is clean".
#
# Read into the array with a loop rather than `mapfile`: macOS ships bash 3.2,
# which has neither mapfile nor a `${#arr[@]}` that survives `set -u` on an
# EMPTY array. Counting as we go sidesteps both, and keeps the expansion below
# safe because it only runs once COUNT proves the array is non-empty.
# Capture discovery separately: process substitution hides git's exit status.
# NUL records preserve every legal path, including whitespace and newlines.
INVENTORY="$WORK/inventory"
if ! git ls-files -z -- '*.sh' > "$INVENTORY"; then
    echo 'ERROR: cannot discover tracked shell inputs; refusing partial inventory.' >&2
    exit 1
fi
FILES=()
COUNT=0
while IFS= read -r -d '' f; do
    FILES+=("$f")
    COUNT=$((COUNT + 1))
done < "$INVENTORY"

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

# Print one section of the key: the label, each path, and each file's raw bytes
# as a hash.
key_section() { # <label> <nul-separated-path-list>
    printf '\n%s\n' "$1"
    cat "$2" || return 1
    [ -s "$2" ] || return 0
    xargs -0 git hash-object --no-filters -- < "$2"
}

# Print the key of every input ShellCheck reads now, or fail. The checked
# section is the discovery list itself, so a key never covers a script this
# run did not check. An untracked script, ignored or not, is only read, when
# a tracked one sources it. Staging a script moves it into the checked
# section, so it changes the key. A nested rc file counts under either of its
# names, since ShellCheck uses the nearest one. SHELLCHECK_OPTS can name files
# no listing here finds, so a run that sets it gets no key. The key rests on
# the same rule as discovery: every shell file here ends in .sh.
pass_key() {
    local config="$WORK/key.config" untracked="$WORK/key.untracked" material="$WORK/key.material"
    [ -z "${SHELLCHECK_OPTS:-}" ] || return 1
    git ls-files -z --cached -- '*shellcheckrc' > "$config" || return 1
    git ls-files -z --others -- '*.sh' '*shellcheckrc' > "$untracked" || return 1
    {
        shellcheck --version || return 1
        key_section checked "$INVENTORY" || return 1
        key_section config "$config" || return 1
        key_section untracked "$untracked" || return 1
    } > "$material" 2> /dev/null
    git hash-object --stdin < "$material"
}

# Add <key> to the record, keeping the newest PASS_RECORD_KEEP keys. A record
# that cannot be written only costs the next run a full check.
record_pass() { # <key>
    mkdir -p "$(dirname "$PASS_RECORD")" 2> /dev/null || return 0
    {
        grep -vxF "$1" "$PASS_RECORD" 2> /dev/null | tail -n "$((PASS_RECORD_KEEP - 1))"
        echo "$1"
    } > "$PASS_RECORD.$$" 2> /dev/null && mv -f "$PASS_RECORD.$$" "$PASS_RECORD" && return 0
    rm -f "$PASS_RECORD.$$"
}

KEY="$(pass_key)" || KEY=""
if [ -n "$KEY" ] && [ -z "${LINT_SHELL_NO_CACHE:-}" ] && grep -qxF "$KEY" "$PASS_RECORD" 2> /dev/null; then
    printf '%s✓%s shellcheck: %d files, no findings (unchanged since the last pass; LINT_SHELL_NO_CACHE=1 forces a run)\n' \
        "$C_GREEN" "$C_RESET" "$COUNT"
    exit 0
fi

echo "Linting $COUNT shell scripts with $(shellcheck --version | awk '/^version:/ { print "shellcheck " $2 }'), $JOBS at once..."

BATCHES=0
i=0
while [ "$i" -lt "$COUNT" ]; do
    printf '%s\0' "${FILES[@]:i:SHELLCHECK_BATCH_SIZE}" > "$WORK/$BATCHES.list"
    BATCHES=$((BATCHES + 1))
    i=$((i + SHELLCHECK_BATCH_SIZE))
done

# Check batch <n>. The status file is written last: a batch that dies leaves
# none, and a missing status fails the gate. ShellCheck exits 1 for findings.
# Any other non-zero status means it gave no verdict.
run_batch() { # <n>
    local files=() f
    while IFS= read -r -d '' f; do
        files+=("$f")
    done < "$WORK/$1.list"
    shellcheck ${C_GREEN:+--color=always} "${files[@]}" > "$WORK/$1.out" 2>&1
    echo "$?" > "$WORK/$1.rc"
}

# Drop the batches that have ended from RUNNING, the pids still at work.
RUNNING=""
forget_ended_batches() {
    local pid alive=""
    for pid in $RUNNING; do
        kill -0 "$pid" 2> /dev/null && alive="$alive $pid"
    done
    RUNNING=$alive
}

# Whether a further batch may start beside the running ones.
room_for_another_batch() { # <running-count>
    [ "$1" -lt "$JOBS" ] || return 1
    [ -z "$MEMORY_CHECKED" ] || lint_shell_has_room "$RUNNING"
}

# Start the batches in order. One batch always runs, as the single process
# always did, so a host short of memory still finishes.
n=0
while [ "$n" -lt "$BATCHES" ]; do
    forget_ended_batches
    # shellcheck disable=SC2086 # word splitting counts the pids
    set -- $RUNNING
    if [ "$#" -eq 0 ] || room_for_another_batch "$#"; then
        run_batch "$n" &
        RUNNING="$RUNNING $!"
        n=$((n + 1))
    else
        sleep "$DISPATCH_POLL_SECS"
    fi
done
wait

FINDINGS=""
NO_VERDICT=""
n=0
while [ "$n" -lt "$BATCHES" ]; do
    [ -f "$WORK/$n.out" ] && cat "$WORK/$n.out"
    case "$(cat "$WORK/$n.rc" 2> /dev/null)" in
        0) ;;
        1) FINDINGS=1 ;;
        '')
            echo "ERROR: ShellCheck batch $((n + 1)) of $BATCHES ended without a result." >&2
            NO_VERDICT=1
            ;;
        *)
            echo "ERROR: ShellCheck batch $((n + 1)) of $BATCHES ended with status $(cat "$WORK/$n.rc"), which is not a verdict." >&2
            NO_VERDICT=1
            ;;
    esac
    n=$((n + 1))
done

if [ -z "$FINDINGS" ] && [ -z "$NO_VERDICT" ]; then
    # Record only inputs that stayed the same for the whole run.
    if [ -n "$KEY" ] && [ "$(pass_key)" = "$KEY" ]; then
        record_pass "$KEY"
    fi
    printf '%s✓%s shellcheck: %d files, no findings\n' "$C_GREEN" "$C_RESET" "$COUNT"
    exit 0
fi

if [ -n "$NO_VERDICT" ]; then
    printf '\n%s✗%s shellcheck did not finish (see the ERROR lines above).\n' "$C_RED" "$C_RESET" >&2
    [ -n "$FINDINGS" ] || exit 1
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
