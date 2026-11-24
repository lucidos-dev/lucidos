#!/bin/bash
# Tests for scripts/lint-shell.sh: it runs ShellCheck in bounded batches, some
# at once, and one failing batch fails the gate without hiding any other
# batch's findings. A pass is reused only while every input is unchanged.
#
# Hermetic: the script runs from a copy inside a throwaway git repo, and a stub
# `shellcheck` on PATH records the argv of every call. The real ShellCheck
# never runs, so the outcome does not drift with the real tree. Every run names
# its job count or supplies a stand-in job rule, so no test reads the host's
# cores or memory.
#
# Run: ./scripts/lib/lint_shell_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LINT="$SCRIPT_DIR/../lint-shell.sh"
# shellcheck source=scripts/lib/lint_shell_jobs.sh
source "$SCRIPT_DIR/lint_shell_jobs.sh"
# shellcheck source=scripts/lib/shellcheck_pin.sh
source "$SCRIPT_DIR/shellcheck_pin.sh"

# The process list every test reads in place of the host's, in the layout of
# _lint_shell_process_list. The default holds no ShellCheck.
PROCESS_LIST='4096 1 bash'
_lint_shell_process_list() { printf '%s\n' "$PROCESS_LIST"; }

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# Pin read from the script, so the test never restates the batch size.
BATCH_SIZE="$(sed -n 's/^SHELLCHECK_BATCH_SIZE=\([0-9][0-9]*\)$/\1/p' "$LINT")"
if [ -z "$BATCH_SIZE" ]; then
    echo "FAIL: lint-shell.sh defines no SHELLCHECK_BATCH_SIZE=<n> line"
    exit 1
fi

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
export SHELLCHECK_MIN_VERSION LUCIDOS_NO_TOOL_DOWNLOAD=1 XDG_CACHE_HOME="$SANDBOX/cache"

# The stub writes one line per call: the file arguments, space separated. A
# file whose name starts with "bad" yields a finding and exit status 1.
# SHELLCHECK_SLOW names a file whose call sleeps first, and SHELLCHECK_DIES
# names one whose call kills its caller before any result is written.
# SHELLCHECK_KILLED names one whose call is itself killed, as by the OOM killer.
# SHELLCHECK_APPENDS_TO names a file each call edits. SHELLCHECK_EXCLUSIVE
# names a directory each call holds, so two calls at once leave a mark.
mkdir -p "$SANDBOX/bin"
cat > "$SANDBOX/bin/shellcheck" <<'EOF'
#!/bin/bash
if [ "${1:-}" = "--version" ]; then
    echo "version: ${SHELLCHECK_STUB_VERSION:-$SHELLCHECK_MIN_VERSION}"
    exit 0
fi
echo "$*" >> "$SHELLCHECK_CALLS"
[ -n "${SHELLCHECK_APPENDS_TO:-}" ] && echo '# edited during the run' >> "$SHELLCHECK_APPENDS_TO"
if [ -n "${SHELLCHECK_EXCLUSIVE:-}" ]; then
    mkdir "$SHELLCHECK_EXCLUSIVE" 2> /dev/null || touch "$SHELLCHECK_EXCLUSIVE.overlap"
    sleep 0.3
    rmdir "$SHELLCHECK_EXCLUSIVE" 2> /dev/null
fi
rc=0
for f in "$@"; do
    [ -f "$f" ] || { echo "missing file argument: $f"; exit 1; }
    [ "$(basename "$f")" = "${SHELLCHECK_SLOW:-}" ] && sleep 1
    [ "$(basename "$f")" = "${SHELLCHECK_DIES:-}" ] && kill -KILL "$PPID"
    [ "$(basename "$f")" = "${SHELLCHECK_KILLED:-}" ] && kill -KILL "$$"
    case "$(basename "$f")" in
        bad*) echo "In $f line 1: stub finding"; rc=1 ;;
    esac
done
exit "$rc"
EOF
chmod +x "$SANDBOX/bin/shellcheck"

# A fresh repo holding a copy of lint-shell.sh, .shellcheckrc, and the given
# number of clean scripts plus any named extras.
make_repo() { # <clean-count> [extra-file...]
    REPO="$(mktemp -d "$SANDBOX/repo.XXXXXX")"
    git -C "$REPO" init -q -b main
    mkdir -p "$REPO/scripts"
    cp "$LINT" "$REPO/scripts/lint-shell.sh"
    touch "$REPO/.shellcheckrc"
    local i=1
    while [ "$i" -le "$1" ]; do
        printf '#!/bin/bash\n' > "$REPO/$(printf 'clean%03d.sh' "$i")"
        i=$((i + 1))
    done
    shift
    local extra
    for extra in "$@"; do
        mkdir -p "$REPO/$(dirname "$extra")"
        printf '#!/bin/bash\n' > "$REPO/$extra"
    done
    git -C "$REPO" add -A
    # A sourced fixture stays outside the tracked lint inventory.
    mkdir -p "$REPO/scripts/lib"
    cp "$SCRIPT_DIR/shellcheck_pin.sh" "$REPO/scripts/lib/"
}

# Run the lint in $REPO with one job, or with the given `VAR=value` settings.
run_lint() { # [VAR=value...]
    SHELLCHECK_CALLS="$SANDBOX/calls.$$.$RANDOM"
    : > "$SHELLCHECK_CALLS"
    OUT=$(cd "$REPO" && env -u LINT_SHELL_NO_CACHE -u SHELLCHECK_OPTS \
        LINT_SHELL_JOBS=1 SHELLCHECK_CALLS="$SHELLCHECK_CALLS" \
        PATH="$SANDBOX/bin:$PATH" "$@" bash scripts/lint-shell.sh 2>&1)
    RC=$?
}

call_count() {
    wc -l < "$SHELLCHECK_CALLS" | tr -d ' '
}

# The file arguments of every call, one per line, sorted.
called_files() {
    tr ' ' '\n' < "$SHELLCHECK_CALLS" | sed '/^$/d' | sort
}

# Every call names at most BATCH_SIZE files, and every tracked file is named
# exactly once, in discovery order.
test_batches_are_bounded_and_cover_every_file_once() {
    echo "test: shellcheck gets at most $BATCH_SIZE files per call, every file once"
    make_repo $((BATCH_SIZE * 2 + 3))
    run_lint
    local max=0 n line
    while IFS= read -r line; do
        # shellcheck disable=SC2086 # word splitting counts the file arguments
        set -- $line
        n=$#
        [ "$n" -gt "$max" ] && max=$n
    done < "$SHELLCHECK_CALLS"
    local calls
    calls=$(wc -l < "$SHELLCHECK_CALLS" | tr -d ' ')
    if [ "$max" -le "$BATCH_SIZE" ] && [ "$calls" -eq 3 ]; then
        pass "$calls calls, largest has $max files"
    else
        fail "expected 3 calls of at most $BATCH_SIZE files, got $calls calls, largest $max"
    fi
    local expected got
    expected=$(git -C "$REPO" ls-files '*.sh' | tr '\n' ' ')
    got=$(tr '\n' ' ' < "$SHELLCHECK_CALLS")
    if [ "$got" = "$expected" ]; then
        pass "every file checked once, in discovery order"
    else
        fail "file coverage differs. expected: $expected got: $got"
    fi
    if [ "$RC" -eq 0 ]; then
        pass "clean tree exits 0"
    else
        fail "clean tree exited $RC: $OUT"
    fi
}

# Only the last batch fails: the gate must still fail.
test_failure_in_last_batch_fails_the_gate() {
    echo "test: a finding only in the last batch fails the gate"
    make_repo $((BATCH_SIZE * 2)) scripts/bad_last.sh
    run_lint
    if [ "$RC" -ne 0 ]; then
        pass "exit status $RC"
    else
        fail "exited 0 with a finding in the last batch"
    fi
    case "$OUT" in
        *"In scripts/bad_last.sh line 1: stub finding"*) pass "the last batch's finding is printed" ;;
        *) fail "the last batch's finding is missing: $OUT" ;;
    esac
}

# The first batch fails: later batches still run and their findings print.
test_failure_in_first_batch_does_not_stop_later_batches() {
    echo "test: a failing first batch does not stop later batches"
    make_repo $((BATCH_SIZE * 2)) bad_first.sh zz/bad_late.sh
    run_lint
    local calls
    calls=$(wc -l < "$SHELLCHECK_CALLS" | tr -d ' ')
    if [ "$RC" -ne 0 ] && [ "$calls" -eq 3 ]; then
        pass "exit status $RC after all $calls batches"
    else
        fail "expected non-zero exit after 3 batches, got exit $RC after $calls"
    fi
    case "$OUT" in
        *"In bad_first.sh line 1"*"In zz/bad_late.sh line 1"*) pass "both findings print, in order" ;;
        *) fail "expected both findings in order: $OUT" ;;
    esac
}

# The Makefile must collect all static failures, including after ShellCheck
# fails. These checker stubs never inspect the host or compile anything.
test_static_failure_aggregation() {
    echo "test: static lint collects every checker failure"
    make_repo 0
    cp "$SCRIPT_DIR/../../Makefile" "$REPO/Makefile"
    local checker
    for checker in lint-shell check-eval-not-a-test check-knowhow-refs check-retired-terms check-audit-scans; do
        printf '#!/bin/sh\necho %s >> calls\nexit 1\n' "$checker" > "$REPO/scripts/$checker.sh"
        chmod +x "$REPO/scripts/$checker.sh"
    done
    local rc=0
    (cd "$REPO" && make --no-print-directory lint-static > out 2>&1) || rc=$?
    if [ "$rc" -eq 0 ]; then
        fail "static lint accepted failing checkers"
    elif [ "$(sort -u "$REPO/calls" | wc -l | tr -d ' ')" -ne 5 ]; then
        fail "static lint omitted a checker after another checker failed"
    else
        pass "all five checkers run and static lint fails"
    fi
    : > "$REPO/calls"
    rc=0
    (cd "$REPO" && make --no-print-directory lint-static 'LINT_STATIC_TARGETS=lint-shell lint-eval' > out 2>&1) || rc=$?
    if [ "$rc" -ne 0 ] && [ "$(sort -u "$REPO/calls" | wc -l | tr -d ' ')" -eq 2 ]; then
        pass "after independent gates, both remaining static failures run without repeating knowhow"
    else
        fail "after-gates static selection omitted checks or repeated knowhow"
    fi
}

# A failed inventory must not certify the subset git happened to print.
test_discovery_failure_is_closed() {
    echo "test: failed discovery rejects partial output"
    make_repo 2
    cat > "$SANDBOX/bin/git" <<'EOF'
#!/bin/sh
printf 'clean001.sh\n'
exit 1
EOF
    chmod +x "$SANDBOX/bin/git"
    run_lint
    rm "$SANDBOX/bin/git"
    if [ "$RC" -ne 0 ] && [ ! -s "$SHELLCHECK_CALLS" ]; then
        pass "failed inventory prevents analysis of a misleading subset"
    else
        fail "failed inventory accepted or analyzed partial output: $OUT"
    fi
}

test_unusual_paths_are_preserved() {
    echo "test: whitespace and newline paths remain single file arguments"
    make_repo 0 'space name.sh' "line
break.sh"
    run_lint
    if [ "$RC" -eq 0 ] && [ "$(wc -l < "$SHELLCHECK_CALLS" | tr -d ' ')" -eq 2 ]; then
        pass "unusual paths survive NUL inventory"
    else
        fail "unusual paths changed discovery: $OUT"
    fi
}

test_parallel_make_respects_static_barrier() {
    echo "test: parallel make cannot run Rust after static lint fails"
    make_repo 0
    cp "$SCRIPT_DIR/../../Makefile" "$REPO/Makefile"
    cat > "$REPO/barrier.mk" <<'EOF'
.PHONY: barrier-static-failure
lint-static: barrier-static-failure
barrier-static-failure:
	@sleep 1; false
lint-fmt:
	@:
lint-rust:
	@touch compiled
EOF
    # Override the static target recipe so the fixture runs no real checker.
    printf 'lint-static:\n\t@:\n' >> "$REPO/barrier.mk"
    local rc=0
    (cd "$REPO" && make -j8 -f Makefile -f barrier.mk 'MAKE=make -f Makefile -f barrier.mk' lint > out 2>&1) || rc=$?
    if [ "$rc" -ne 0 ] && [ ! -e "$REPO/compiled" ]; then
        pass "parallel lint stops before Rust when static lint fails"
    else
        fail "parallel lint breached static barrier"
    fi
    printf 'barrier-static-failure:\n\t@:\n' >> "$REPO/barrier.mk"
    rc=0
    (cd "$REPO" && make -j8 -f Makefile -f barrier.mk 'MAKE=make -f Makefile -f barrier.mk' lint > out 2>&1) || rc=$?
    if [ "$rc" -eq 0 ] && [ -e "$REPO/compiled" ]; then
        pass "parallel lint reaches Rust after static lint passes"
    else
        fail "passing static lint did not reach Rust"
    fi
}

# Several jobs still name every tracked file exactly once.
test_parallel_jobs_cover_every_file_once() {
    echo "test: three jobs check every file exactly once"
    make_repo $((BATCH_SIZE * 5 + 1))
    run_lint LINT_SHELL_JOBS=3
    if [ "$RC" -eq 0 ] && [ "$(call_count)" -eq 6 ]; then
        pass "6 batches ran and the clean tree exits 0"
    else
        fail "expected exit 0 after 6 batches, got exit $RC after $(call_count): $OUT"
    fi
    if [ "$(called_files)" = "$(git -C "$REPO" ls-files '*.sh' | sort)" ]; then
        pass "every file checked once"
    else
        fail "file coverage differs under three jobs"
    fi
}

# The first batch finishes last, and its finding still prints first.
test_parallel_output_keeps_discovery_order() {
    echo "test: findings print in discovery order, whatever order the jobs finish in"
    make_repo $((BATCH_SIZE * 2)) bad_first.sh zz/bad_late.sh
    run_lint LINT_SHELL_JOBS=3 SHELLCHECK_SLOW=bad_first.sh
    if [ "$RC" -ne 0 ] && [ "$(call_count)" -eq 3 ]; then
        pass "exit status $RC after all 3 batches"
    else
        fail "expected non-zero exit after 3 batches, got exit $RC after $(call_count)"
    fi
    case "$OUT" in
        *"In bad_first.sh line 1"*"In zz/bad_late.sh line 1"*) pass "the slow first batch still prints first" ;;
        *) fail "expected both findings in discovery order: $OUT" ;;
    esac
}

# A batch that dies leaves no result, and a missing result is never a pass.
test_a_batch_without_a_result_fails_the_gate() {
    echo "test: a batch that dies before reporting fails the gate"
    make_repo $((BATCH_SIZE * 2))
    run_lint LINT_SHELL_JOBS=2 SHELLCHECK_DIES=clean001.sh
    if [ "$RC" -ne 0 ]; then
        pass "exit status $RC"
    else
        fail "exited 0 although one batch reported nothing: $OUT"
    fi
    run_lint
    if [ "$RC" -eq 0 ] && [ "$(call_count)" -gt 0 ]; then
        pass "the failed run recorded nothing, so the next run checks again"
    else
        fail "expected a full run after a failed one, got exit $RC with $(call_count) calls"
    fi
}

# A killed ShellCheck gave no verdict, and the gate says so by name.
test_a_killed_shellcheck_is_not_reported_as_a_finding() {
    echo "test: a ShellCheck that is killed fails the gate as a crash, not a finding"
    make_repo $((BATCH_SIZE * 2))
    run_lint LINT_SHELL_JOBS=2 SHELLCHECK_KILLED=clean001.sh
    if [ "$RC" -ne 0 ]; then
        pass "exit status $RC"
    else
        fail "exited 0 although one ShellCheck was killed: $OUT"
    fi
    case "$OUT" in
        *"reported findings"*) fail "the crash reads as a finding: $OUT" ;;
        *"batch 1 of "*" ended with status 137"*) pass "the output names the batch and its status" ;;
        *) fail "the output does not name the killed batch: $OUT" ;;
    esac
    run_lint
    if [ "$RC" -eq 0 ] && [ "$(call_count)" -gt 0 ]; then
        pass "the failed run recorded nothing, so the next run checks again"
    else
        fail "expected a full run after a failed one, got exit $RC with $(call_count) calls"
    fi
}

test_a_bad_job_count_is_refused() {
    echo "test: a job count that is not a positive number is refused"
    make_repo 2
    local bad
    for bad in 0 many -3; do
        run_lint "LINT_SHELL_JOBS=$bad"
        if [ "$RC" -ne 0 ] && [ "$(call_count)" -eq 0 ]; then
            pass "LINT_SHELL_JOBS=$bad exits $RC before any call"
        else
            fail "LINT_SHELL_JOBS=$bad ran: exit $RC, $(call_count) calls"
        fi
    done
}

# Expect the next run in $REPO to call ShellCheck again, for the named reason.
expect_rerun() { # <reason> [VAR=value...]
    local reason=$1
    shift
    run_lint "$@"
    if [ "$RC" -eq 0 ] && [ "$(call_count)" -gt 0 ]; then
        pass "$reason runs ShellCheck again"
    else
        fail "$reason reused a pass: exit $RC, $(call_count) calls"
    fi
}

# Expect the next run in $REPO to reuse the pass the last run recorded.
expect_reuse() { # <reason> [VAR=value...]
    local reason=$1
    shift
    run_lint "$@"
    if [ "$RC" -eq 0 ] && [ "$(call_count)" -eq 0 ]; then
        pass "$reason reuses the pass"
    else
        fail "$reason did not reuse the pass: exit $RC, $(call_count) calls"
    fi
}

# A pass is reused only while every input ShellCheck reads is byte identical.
test_a_pass_is_reused_only_for_identical_inputs() {
    echo "test: a pass is reused only while every input is unchanged"
    make_repo 3 scripts/lib/helper.sh
    expect_rerun "the first run"
    expect_reuse "an unchanged tree"
    local tracked
    tracked=$(git -C "$REPO" ls-files '*.sh' | wc -l | tr -d ' ')
    case "$OUT" in
        *"$tracked files"*) pass "the reused pass still names the file count" ;;
        *) fail "the reused pass does not name the file count: $OUT" ;;
    esac

    echo 'echo edited' >> "$REPO/clean002.sh"
    expect_rerun "an edited script"
    expect_reuse "the edited tree, once it passed"

    printf '#!/bin/bash\n' > "$REPO/scripts/lib/untracked_helper.sh"
    expect_rerun "a new untracked script, which a tracked one could source"

    git -C "$REPO" add scripts/lib/untracked_helper.sh
    expect_rerun "that script once staged, since no run has checked it"

    echo 'scripts/lib/ignored_helper.sh' > "$REPO/.gitignore"
    printf '#!/bin/bash\n' > "$REPO/scripts/lib/ignored_helper.sh"
    expect_rerun "a new ignored script, which a tracked one could source"
    echo 'echo edited' >> "$REPO/scripts/lib/ignored_helper.sh"
    expect_rerun "an edited ignored script"

    mkdir -p "$REPO/scripts/nested"
    echo 'disable=SC2086' > "$REPO/scripts/nested/.shellcheckrc"
    expect_rerun "a nested .shellcheckrc"
    echo 'disable=SC2086' > "$REPO/scripts/nested/shellcheckrc"
    expect_rerun "a nested shellcheckrc, the name without the dot"

    printf '#!/bin/bash\r\n' > "$REPO/clean001.sh"
    git -C "$REPO" config core.autocrlf input
    expect_rerun "changed line endings, which a git filter would hide"

    echo 'disable=SC2034' >> "$REPO/.shellcheckrc"
    expect_rerun "a changed .shellcheckrc"

    expect_rerun "another ShellCheck version" SHELLCHECK_STUB_VERSION=9.9.9
    expect_reuse "the same other version" SHELLCHECK_STUB_VERSION=9.9.9
    expect_rerun "SHELLCHECK_OPTS, which can name files the key cannot see" SHELLCHECK_OPTS=--severity=error
    expect_rerun "the same SHELLCHECK_OPTS again" SHELLCHECK_OPTS=--severity=error
    expect_reuse "the first version again"
    echo 'echo edited again' >> "$REPO/clean003.sh"
    expect_rerun "an edited script under SHELLCHECK_OPTS" SHELLCHECK_OPTS=--severity=error
    expect_rerun "that tree without SHELLCHECK_OPTS, since the run under it recorded nothing"
    expect_rerun "LINT_SHELL_NO_CACHE=1" LINT_SHELL_NO_CACHE=1
}

# Only a pass is recorded: a tree with a finding is checked on every run.
test_a_failing_run_is_never_reused() {
    echo "test: a run with a finding records nothing"
    make_repo 2 bad_one.sh
    run_lint
    local first=$RC
    run_lint
    if [ "$first" -ne 0 ] && [ "$RC" -ne 0 ] && [ "$(call_count)" -gt 0 ]; then
        pass "the second run checks again and fails again"
    else
        fail "expected two failing runs, got exit $first then exit $RC with $(call_count) calls"
    fi
}

# ShellCheck may have read either version of a file edited during the run.
test_a_run_with_changing_inputs_records_nothing() {
    echo "test: a pass is not recorded when an input changed during the run"
    make_repo 2
    run_lint "SHELLCHECK_APPENDS_TO=$REPO/clean001.sh"
    printf '#!/bin/bash\n' > "$REPO/clean001.sh"
    expect_rerun "the tree as it was before the run"
}

# A `git` that stages <file> right after its first `ls-files`, which is the
# discovery call. Later calls pass straight through.
make_git_that_stages_after_discovery() { # <file>
    mkdir -p "$SANDBOX/gitbin"
    rm -f "$SANDBOX/gitbin/staged"
    cat > "$SANDBOX/gitbin/git" <<EOF
#!/bin/bash
if [ "\${1:-}" = ls-files ] && [ ! -e "$SANDBOX/gitbin/staged" ]; then
    "$(command -v git)" "\$@" || exit
    touch "$SANDBOX/gitbin/staged"
    exec "$(command -v git)" add "$1"
fi
exec "$(command -v git)" "\$@"
EOF
    chmod +x "$SANDBOX/gitbin/git"
}

# The key must cover the scripts this run checked, never a later listing.
test_a_script_staged_after_discovery_is_not_covered_by_the_pass() {
    echo "test: a script staged after discovery is checked by the next run"
    make_repo 2
    printf '#!/bin/bash\n' > "$REPO/bad_late.sh"
    make_git_that_stages_after_discovery bad_late.sh
    run_lint "PATH=$SANDBOX/gitbin:$SANDBOX/bin:$PATH"
    if [ "$RC" -eq 0 ] && ! called_files | grep -qx bad_late.sh; then
        pass "the first run passes without the late script"
    else
        fail "expected a pass that never saw bad_late.sh: exit $RC: $OUT"
    fi
    run_lint
    if [ "$RC" -ne 0 ] && called_files | grep -qx bad_late.sh; then
        pass "the next run checks the late script and reports its finding"
    else
        fail "the late script rode on the recorded pass: exit $RC, $(call_count) calls"
    fi
}

# The stand-in job rule: three jobs, and room only while the flag file exists.
# It logs the batch pids each room question carries.
stub_job_rule() { # <room-flag>
    mkdir -p "$REPO/scripts/lib"
    cat > "$REPO/scripts/lib/lint_shell_jobs.sh" <<EOF
lint_shell_jobs() { echo 3; }
lint_shell_has_room() { echo "\$1" >> "$1.asked"; [ -e "$1" ]; }
EOF
}

# With the job count taken from the host, memory is read again per batch.
test_batches_wait_for_memory_but_one_always_runs() {
    echo "test: without room, batches run one at a time and all still run"
    make_repo $((BATCH_SIZE * 2))
    stub_job_rule "$REPO/.room"
    git -C "$REPO" add -A
    local busy="$SANDBOX/busy.$RANDOM"
    run_lint LINT_SHELL_JOBS= "SHELLCHECK_EXCLUSIVE=$busy" LINT_SHELL_NO_CACHE=1
    if [ "$RC" -eq 0 ] && [ ! -e "$busy.overlap" ] \
        && [ "$(called_files)" = "$(git -C "$REPO" ls-files '*.sh' | sort)" ]; then
        pass "every file checked, never two batches at once"
    else
        fail "expected a serial full run: exit $RC, $(call_count) calls: $OUT"
    fi
    if [ "$(awk '{ print NF }' "$REPO/.room.asked" | sort -u)" = 1 ]; then
        pass "each room question names the one batch already running"
    else
        fail "expected one batch pid per room question: $(tr '\n' '|' < "$REPO/.room.asked")"
    fi
    touch "$REPO/.room"
    run_lint LINT_SHELL_JOBS= "SHELLCHECK_EXCLUSIVE=$busy" LINT_SHELL_NO_CACHE=1
    if [ "$RC" -eq 0 ] && [ -e "$busy.overlap" ]; then
        pass "with room, batches run at once"
    else
        fail "expected overlapping batches with room: exit $RC: $OUT"
    fi
    if awk 'NF == 2 { found = 1 } END { exit !found }' "$REPO/.room.asked"; then
        pass "the room question names every batch this run has started"
    else
        fail "no room question named 2 batch pids: $(tr '\n' '|' < "$REPO/.room.asked")"
    fi
}

# Print <n> GB the way the spare reading prints it.
gb() { printf '%d.00' "$1"; }

# A ShellCheck takes its memory late, so its unclaimed grant is not spare.
test_spare_memory_leaves_out_what_running_shellchecks_will_take() {
    echo "test: spare memory leaves out $LINT_SHELL_GB_PER_JOB GB per running ShellCheck, less what it holds"
    local per=$LINT_SHELL_GB_PER_JOB avail=$((LINT_SHELL_GB_PER_JOB * 4)) gb_kb=1048576
    local half=$((gb_kb * LINT_SHELL_GB_PER_JOB / 2))
    local name own want list got
    while IFS='|' read -r name own want list; do
        got="$(printf '%b' "$list" | lint_shell_spare_gb_for "$avail" "$own")"
        if [ "$got" = "$want" ]; then
            pass "$name: ${got:-unreadable}"
        else
            fail "$name: expected '${want:-unreadable}', got '${got:-unreadable}'"
        fi
    done <<EOF
no ShellCheck running||$(gb "$avail")|4096 1 bash\n
two that just started||$(gb $((avail - per * 2)))|0 1 shellcheck\n0 1 shellcheck\n9 1 bash\n
one that holds half its grant||$(gb $((avail - per / 2)))|$half 1 shellcheck\n
one that holds more than its grant||$(gb "$avail")|$((gb_kb * per * 2)) 1 shellcheck\n
a full path, as macOS prints it||$(gb $((avail - per)))|0 1 /opt/homebrew/bin/shellcheck\n
a path with a space in it||$(gb $((avail - per)))|0 1 /Users/me/my tools/shellcheck\n
another program with a similar name||$(gb "$avail")|0 1 shellcheck-wrapper\n0 1 /usr/bin/myshellcheck\n
two own batches not listed yet|700 701|$(gb $((avail - per * 2)))|4096 1 bash\n
two own batches, both listed|700 701|$(gb $((avail - per * 2)))|0 700 shellcheck\n0 701 shellcheck\n
an own batch not listed yet, beside two of another session|700|$(gb $((avail - per * 3)))|0 1 shellcheck\n0 1 shellcheck\n
an own batch that holds half its grant|700|$(gb $((avail - per / 2)))|$half 700 shellcheck\n
more spoken for than available||-$(gb "$per")|$(printf '0 1 shellcheck\\n%.0s' 1 2 3 4 5)
rows with no executable column|||0 1\n4096 1\n
an own batch pid that is not a number|x||4096 1 bash\n
a process list that cannot be read|||
EOF
    got="$(printf '0 1 shellcheck\n' | lint_shell_spare_gb_for unreadable "")"
    if [ -z "$got" ]; then
        pass "unreadable available memory: unreadable"
    else
        fail "unreadable available memory printed '$got'"
    fi
}

test_room_follows_spare_memory() {
    echo "test: a further batch needs $LINT_SHELL_GB_TO_START GB spare"
    local spare want got
    while read -r spare want; do
        got=no
        lint_shell_room_for "$spare" && got=yes
        if [ "$got" = "$want" ]; then
            pass "'$spare' GB spare: room $got"
        else
            fail "'$spare' GB spare: expected room $want, got $got"
        fi
    done <<EOF
$LINT_SHELL_GB_TO_START yes
$((LINT_SHELL_GB_TO_START + 1)).5 yes
$((LINT_SHELL_GB_TO_START - 1)).9 no
unreadable no
EOF
    if HOST_AVAIL_GB_OVERRIDE=$((LINT_SHELL_GB_TO_START * 2)) lint_shell_has_room 700 \
        && ! HOST_AVAIL_GB_OVERRIDE=0.5 lint_shell_has_room 700; then
        pass "the host reading feeds the same rule"
    else
        fail "lint_shell_has_room ignores the host reading"
    fi
    local enough=$((LINT_SHELL_GB_TO_START + LINT_SHELL_GB_PER_JOB))
    if HOST_AVAIL_GB_OVERRIDE=$enough lint_shell_has_room 700 \
        && ! HOST_AVAIL_GB_OVERRIDE=$enough lint_shell_has_room '700 701' \
        && ! PROCESS_LIST='0 1 shellcheck' HOST_AVAIL_GB_OVERRIDE=$enough lint_shell_has_room 700; then
        pass "running ShellChecks, own or another session's, take from the room"
    else
        fail "lint_shell_has_room ignores what running ShellChecks will take"
    fi
    if ! PROCESS_LIST='' HOST_AVAIL_GB_OVERRIDE=$((LINT_SHELL_GB_TO_START * 2)) lint_shell_has_room 700; then
        pass "a process list that cannot be read: no room"
    else
        fail "an unreadable process list still gave room"
    fi
    if ! (
        _lint_shell_process_list() { echo '4096 1 bash'; return 1; }
        HOST_AVAIL_GB_OVERRIDE=$((LINT_SHELL_GB_TO_START * 2)) lint_shell_has_room 700
    ); then
        pass "a process list that fails part way: no room"
    else
        fail "a failed process list still gave room"
    fi
}

# The record lives in the checkout, so an identical checkout elsewhere misses.
test_a_pass_stays_in_its_checkout() {
    echo "test: a pass in one checkout is not reused in an identical one"
    make_repo 2
    expect_rerun "the first checkout"
    make_repo 2
    expect_rerun "an identical second checkout"
}

test_job_count_follows_cores_and_memory() {
    echo "test: one job per $LINT_SHELL_GB_PER_JOB GB spare, capped by cores and by $LINT_SHELL_MAX_JOBS"
    local ncpu spare want got
    while read -r ncpu spare want; do
        got="$(lint_shell_jobs_for "$ncpu" "$spare")"
        if [ "$got" = "$want" ]; then
            pass "$ncpu cores, '$spare' GB spare: $got jobs"
        else
            fail "$ncpu cores, '$spare' GB spare: expected $want jobs, got $got"
        fi
    done <<EOF
4 $((LINT_SHELL_GB_PER_JOB * 3)).5 3
2 $((LINT_SHELL_GB_PER_JOB * 3)) 2
64 $((LINT_SHELL_GB_PER_JOB * LINT_SHELL_MAX_JOBS * 2)) $LINT_SHELL_MAX_JOBS
8 0.4 1
8 unreadable 1
unreadable 16 1
EOF
    got="$(HOST_NCPU_OVERRIDE=4 HOST_AVAIL_GB_OVERRIDE=$((LINT_SHELL_GB_PER_JOB * 2)) lint_shell_jobs)"
    if [ "$got" = 2 ]; then
        pass "the host reading feeds the same rule"
    else
        fail "expected 2 jobs from the host reading, got $got"
    fi
    got="$(PROCESS_LIST='0 1 shellcheck' HOST_NCPU_OVERRIDE=4 HOST_AVAIL_GB_OVERRIDE=$((LINT_SHELL_GB_PER_JOB * 3)) lint_shell_jobs)"
    if [ "$got" = 2 ]; then
        pass "another session's running ShellCheck lowers the count"
    else
        fail "expected 2 jobs beside one running ShellCheck, got $got"
    fi
    got="$(PROCESS_LIST='' HOST_NCPU_OVERRIDE=4 HOST_AVAIL_GB_OVERRIDE=$((LINT_SHELL_GB_PER_JOB * 3)) lint_shell_jobs 2>&1)"
    case "$got" in
        *"cannot read spare memory"*$'\n'1) pass "a process list that cannot be read: 1 job, and a line that says why" ;;
        *) fail "expected a note and 1 job without a process list, got '$got'" ;;
    esac
}

test_parallel_make_respects_static_barrier

test_unusual_paths_are_preserved

test_discovery_failure_is_closed

test_static_failure_aggregation

test_batches_are_bounded_and_cover_every_file_once
test_failure_in_last_batch_fails_the_gate
test_failure_in_first_batch_does_not_stop_later_batches

test_parallel_jobs_cover_every_file_once
test_parallel_output_keeps_discovery_order
test_a_batch_without_a_result_fails_the_gate
test_a_killed_shellcheck_is_not_reported_as_a_finding
test_a_bad_job_count_is_refused
test_a_pass_is_reused_only_for_identical_inputs
test_a_script_staged_after_discovery_is_not_covered_by_the_pass
test_a_failing_run_is_never_reused
test_a_pass_stays_in_its_checkout
test_a_run_with_changing_inputs_records_nothing
test_batches_wait_for_memory_but_one_always_runs
test_room_follows_spare_memory
test_spare_memory_leaves_out_what_running_shellchecks_will_take
test_job_count_follows_cores_and_memory

echo
echo "lint_shell_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
