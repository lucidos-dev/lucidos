#!/bin/bash
# Tests for scripts/confinement-tests.sh.
#
# Hermetic. `uname`, `sandbox-exec`, `cargo` and `lucidos` are stubs on PATH
# that log their calls, so nothing builds and no engine is asked. The script
# runs from a copy beside a stub build slot, which logs and runs its command.
# The last test scans the tree for confinement tests missing their #[ignore].
#
# Run: ./scripts/lib/confinement_tests_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
assert_eq() { # <expected> <actual> <label>
    if [ "$1" = "$2" ]; then pass "$3"; else fail "$3 (expected '$1', got '$2')"; fi
}

STUBS="$SANDBOX/bin"
LOG="$SANDBOX/calls"
mkdir -p "$STUBS"
stub() { # <name> <body>
    printf '#!/bin/bash\necho "%s $*" >> "%s"\n%s\n' "$1" "$LOG" "$2" > "$STUBS/$1"
    chmod +x "$STUBS/$1"
}
stub cargo "exit 0"
stub lucidos "exit 0"

COPY="$SANDBOX/root"
mkdir -p "$COPY/scripts"
cp "$ROOT/scripts/confinement-tests.sh" "$COPY/scripts/"
cat > "$COPY/scripts/with-build-slot.sh" << EOF
#!/bin/bash
echo "slot \$*" >> "$LOG"
while [ "\$1" != "--" ]; do shift; done
shift
exec "\$@"
EOF
chmod +x "$COPY/scripts/with-build-slot.sh"
cat > "$COPY/scripts/e2e-github.sh" << EOF
#!/bin/bash
echo "github \$*" >> "$LOG"
echo "fixture GitHub outcome"
exit "\${GITHUB_RC:-75}"
EOF
chmod +x "$COPY/scripts/e2e-github.sh"

# Run the script with <os> and a seatbelt trial that exits <trial>, passing
# any further words to it. Sets RC and leaves the calls in $LOG.
route() { # <os> <trial> <thread-id or ""> [script args...]
    stub uname "echo $1"
    stub sandbox-exec "exit $2"
    local thread="${3:-}"
    shift $(($# < 3 ? $# : 3))
    : > "$LOG"
    RC=0
    env -u LUCIDOS_AGENT_SANDBOXED -u LUCIDOS_THREAD_ID ${thread:+LUCIDOS_THREAD_ID=$thread} PATH="$STUBS:$PATH" \
        "$COPY/scripts/confinement-tests.sh" "$@" > "$SANDBOX/output" 2>&1 || RC=$?
}
called() { grep -c "^$1 " "$LOG" | tr -d ' '; }

test_linux_runs_here() {
    echo "test: Linux runs in place, with no seatbelt trial"
    route Linux 71
    assert_eq 0 "$RC" "exit"
    assert_eq 2 "$(called cargo)" "both test binaries ran"
    assert_eq 0 "$(called sandbox-exec)" "no trial"
    assert_eq 0 "$(called lucidos)" "no engine route"
}

test_each_test_binary_takes_a_build_slot() {
    echo "test: each cargo run waits for a build slot"
    route Linux 71
    assert_eq 2 "$(called slot)" "two slots for two cargo runs"
}

test_a_sandbox_that_cannot_apply_ran_nothing() {
    echo "test: a host where the sandbox cannot apply exits 75, never a test failure"
    stub cargo 'echo "panicked: the real sandbox is unavailable: no Landlock"; exit 101'
    route Linux 71
    assert_eq 75 "$RC" "nothing ran"
    stub cargo 'echo "assertion failed"; exit 101'
    route Linux 71
    assert_eq 101 "$RC" "a real failure keeps its own exit"
    stub cargo "exit 0"
}

test_linux_routes_only_unavailable() {
    echo "test: only a missing Linux sandbox routes to GitHub, pinned when requested"
    stub cargo 'echo "the real sandbox is unavailable"; exit 101'
    local code
    for code in 0 1 75 76; do
        export GITHUB_RC="$code"
        route Linux 71 "" --commit fixture-commit
        case "$code" in
            0) assert_eq 0 "$RC" "runner pass" ;;
            1) assert_eq 1 "$RC" "runner failure" ;;
            *) assert_eq 75 "$RC" "no verdict" ;;
        esac
        if grep -q "fixture GitHub outcome" "$SANDBOX/output"; then pass "GitHub reason is visible"; else fail "missing reason"; fi
        assert_eq "github confinement --commit fixture-commit" "$(grep '^github ' "$LOG")" "pinned GitHub call"
    done
    route Linux 71 "" --here
    assert_eq 75 "$RC" "here never routes"
    assert_eq 0 "$(called github)" "no recursive handoff"
    stub cargo 'echo "assertion failed"; exit 101'
    route Linux 71
    assert_eq 101 "$RC" "local failure"
    assert_eq 0 "$(called github)" "no retry of a test failure"
    stub cargo 'exit 75'
    route Linux 71
    assert_eq 75 "$RC" "unrelated local exit 75"
    assert_eq 0 "$(called github)" "no retry without the probe reason"
    stub cargo 'exit 0'
    route Linux 71
    assert_eq 0 "$(called github)" "a local pass stays local"
    unset GITHUB_RC
}

test_macos_trial_applies_runs_here() {
    echo "test: macOS runs in place when the trial applies, even with a thread"
    route Darwin 0 thread-1
    assert_eq 0 "$RC" "exit"
    assert_eq 2 "$(called cargo)" "both test binaries ran"
    assert_eq 0 "$(called lucidos)" "no engine route"
}

test_macos_refused_in_a_thread_asks_the_engine() {
    echo "test: macOS with a refused trial inside a thread asks the engine"
    route Darwin 71 thread-1
    assert_eq 0 "$RC" "exit follows the CLI"
    assert_eq 0 "$(called cargo)" "nothing ran here"
    assert_eq "lucidos confinement-tests run" "$(grep '^lucidos ' "$LOG")" "the CLI call"
}

test_the_hold_reaches_the_cli() {
    echo "test: --hold-secs reaches the CLI on the engine route"
    route Darwin 71 thread-1 --hold-secs 0
    assert_eq 0 "$RC" "exit"
    assert_eq "lucidos confinement-tests run --hold-secs 0" "$(grep '^lucidos ' "$LOG")" "the CLI call"
}

test_the_hold_is_ignored_in_place() {
    echo "test: --hold-secs changes nothing when the tests run here"
    route Linux 71 thread-1 --hold-secs 0
    assert_eq 0 "$RC" "exit"
    assert_eq 2 "$(called cargo)" "both test binaries ran"
    assert_eq 0 "$(called lucidos)" "no engine route"
}

test_a_run_that_did_not_happen_fails() {
    echo "test: the CLI's not-run exit is the script's exit"
    stub lucidos "exit 75"
    route Darwin 71 thread-1 --hold-secs 0
    stub lucidos "exit 0"
    assert_eq 75 "$RC" "exit"
}

test_a_bad_argument_is_refused() {
    echo "test: an unknown argument or a bare --hold-secs is refused"
    route Darwin 71 thread-1 --hold
    assert_eq 2 "$RC" "unknown argument"
    route Darwin 71 thread-1 --hold-secs
    assert_eq 2 "$RC" "missing value"
    assert_eq 0 "$(called lucidos)" "no engine route"
}

test_macos_refused_without_a_thread_fails() {
    echo "test: macOS with a refused trial and no thread fails"
    route Darwin 71
    assert_eq 1 "$RC" "exit"
    assert_eq 0 "$(called cargo)" "nothing ran"
    assert_eq 0 "$(called lucidos)" "no engine route"
}

test_the_marker_never_picks_the_route() {
    echo "test: the script never reads LUCIDOS_AGENT_SANDBOXED"
    if grep -v '^#' "$ROOT/scripts/confinement-tests.sh" | grep -q LUCIDOS_AGENT_SANDBOXED; then
        fail "the script reads the marker"
    else
        pass "no read of the marker"
    fi
}

# Print "<file>: <fn>" for each test in <files> that applies the real sandbox
# (it calls the probe gate, or builds an enforced engine sandbox) without the
# ignore that names the script.
unignored_confinement_tests() { # <files...>
    # shellcheck disable=SC2016 # an awk program, not shell
    awk '
        /#\[(tokio::)?test/ { in_attrs = 1; ignored = 0 }
        in_attrs && /#\[ignore = "confinement test: \.\/scripts\/confinement-tests\.sh runs it"\]/ { ignored = 1 }
        in_attrs && /fn [a-z_0-9]+/ {
            in_attrs = 0; in_fn = 1; depth = 0; marked = 0
            match($0, /fn [a-z_0-9]+/); name = substr($0, RSTART + 3, RLENGTH - 3)
        }
        in_fn {
            if ($0 ~ /require_sandbox\(\)|lucidos_agent_sandbox::probe\(\)|enforced_for(_test)?\(|resolve_under\(.*Machine::Available/) marked = 1
            depth += gsub(/\{/, "{") - gsub(/\}/, "}")
            if (depth <= 0 && /\}/) {
                in_fn = 0
                if (marked && !ignored) print FILENAME ": " name
            }
        }
    ' "$@"
}

test_the_guard_catches_a_missing_ignore() {
    echo "test: the guard names a sandbox test with no ignore, and spares the rest"
    cat > "$SANDBOX/fixture.rs" << 'EOF'
#[test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn ignored_one() {
    require_sandbox();
}

#[tokio::test]
async fn bare_one() {
    let s = Sandbox::enforced_for_test(Policy { ..Default::default() });
}

#[test]
fn unrelated() {
    assert!(format!("{}", 1).len() == 1);
}
EOF
    assert_eq "$SANDBOX/fixture.rs: bare_one" "$(unignored_confinement_tests "$SANDBOX/fixture.rs")" "only the bare one"
}

test_every_confinement_test_is_ignored() {
    echo "test: every test in the tree that applies the real sandbox carries the confinement ignore"
    local files count
    files="$(git -C "$ROOT" ls-files 'crates/*.rs' | sed "s|^|$ROOT/|")"
    # shellcheck disable=SC2086 # one path per word; tracked paths hold no spaces
    assert_eq "" "$(unignored_confinement_tests $files)" "no confinement test without its ignore"
    count="$(git -C "$ROOT" grep -c 'confinement test: ./scripts/confinement-tests.sh runs it' -- 'crates/*.rs' |
        awk -F: '{ n += $2 } END { print n }')"
    assert_eq 11 "$count" "the eleven known confinement tests are ignored"
}

test_linux_runs_here
test_each_test_binary_takes_a_build_slot
test_a_sandbox_that_cannot_apply_ran_nothing
test_linux_routes_only_unavailable
test_macos_trial_applies_runs_here
test_macos_refused_in_a_thread_asks_the_engine
test_the_hold_reaches_the_cli
test_the_hold_is_ignored_in_place
test_a_run_that_did_not_happen_fails
test_a_bad_argument_is_refused
test_macos_refused_without_a_thread_fails
test_the_marker_never_picks_the_route
test_the_guard_catches_a_missing_ignore
test_every_confinement_test_is_ignored

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
