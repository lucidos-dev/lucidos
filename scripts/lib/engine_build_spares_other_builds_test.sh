#!/bin/bash
# Tests that an engine build leaves every other build alone
# (build_or_find_engine in scripts/lib/workspace.sh, ADR 0294). The build
# signals no process and keeps every `.cargo-lock`, so another worktree's
# `cargo check` survives it.
#
# Hermetic by construction (ADR 0025). `kill`, `pkill`, `killall` and `pgrep`
# are shadowed as functions and on PATH, and any call fails the suite. The
# `pgrep` shim lists only the fake cargo spawned here, so a regressed selector
# cannot reach a real process on the host.
#
# Run: ./scripts/lib/engine_build_spares_other_builds_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# shellcheck source=workspace.sh
source "$SCRIPT_DIR/workspace.sh"

SCRATCH="$(mktemp -d)"
SIGNALS="$SCRATCH/signals.log"
: > "$SIGNALS"

# Stands in for the child `cargo check` that `cargo clippy` runs: argv[0] is
# `cargo` and `check` follows. `yes` is a signed system binary, and `exec -a`
# sets its argv[0].
( exec -a cargo yes check --locked --workspace --all-targets >/dev/null 2>&1 ) &
FAKE_CARGO=$!
trap 'builtin kill "$FAKE_CARGO" 2>/dev/null; wait 2>/dev/null; rm -rf "$SCRATCH"' EXIT
sleep 0.5

# PATH shims catch a kill that runs as a program, as `xargs kill` does.
mkdir -p "$SCRATCH/bin"
for tool in kill pkill killall; do
    printf '#!/bin/sh\necho "%s $*" >> "%s"\n' "$tool" "$SIGNALS" > "$SCRATCH/bin/$tool"
done
printf '#!/bin/sh\necho %s\n' "$FAKE_CARGO" > "$SCRATCH/bin/pgrep"
chmod +x "$SCRATCH/bin/"*

CHECKOUT="$SCRATCH/checkout"
mkdir -p "$CHECKOUT/target/debug" "$CHECKOUT/target/release"
touch "$CHECKOUT/target/.cargo-lock" "$CHECKOUT/target/debug/.cargo-lock" \
    "$CHECKOUT/target/release/.cargo-lock"

# Runs the real build_or_find_engine with only the compile, publish and sign
# steps stubbed, under `set -e` as web-dev.sh runs it.
# shellcheck disable=SC2329 # the shims exist for a regressed build to call, and it must not
(
    set -e
    PATH="$SCRATCH/bin:$PATH"
    kill() { echo "kill $*" >> "$SIGNALS"; }
    pkill() { echo "pkill $*" >> "$SIGNALS"; }
    killall() { echo "killall $*" >> "$SIGNALS"; }
    pgrep() { echo "$FAKE_CARGO"; }
    run_engine_cargo_build() { touch "$SCRATCH/compiled"; }
    publish_launch_binaries() { :; }
    published_build_state() { echo current; }
    sign_engine_binary() { :; }
    BUILD=1
    PROJECT_DIR="$CHECKOUT"
    build_or_find_engine
) > "$SCRATCH/build.out" 2>&1
BUILD_RC=$?

echo "test: the build ran to its compile step"
if [ "$BUILD_RC" -eq 0 ] && [ -f "$SCRATCH/compiled" ]; then
    pass "build_or_find_engine reached run_engine_cargo_build and returned 0"
else
    fail "exit $BUILD_RC, compiled=$([ -f "$SCRATCH/compiled" ] && echo yes || echo no): $(cat "$SCRATCH/build.out")"
fi

echo "test: the build sends no signal to any process"
if [ -s "$SIGNALS" ]; then
    fail "it tried to signal: $(tr '\n' ';' < "$SIGNALS")"
else
    pass "no kill, pkill or killall was called"
fi

echo "test: another build's cargo check is still running"
if builtin kill -0 "$FAKE_CARGO" 2>/dev/null; then
    pass "the fake cargo check $FAKE_CARGO survived"
else
    fail "the fake cargo check $FAKE_CARGO is gone"
fi

echo "test: cargo's lock files are left in place"
missing=""
for lock in target/.cargo-lock target/debug/.cargo-lock target/release/.cargo-lock; do
    [ -e "$CHECKOUT/$lock" ] || missing="$missing $lock"
done
if [ -z "$missing" ]; then
    pass "every .cargo-lock is still there"
else
    fail "deleted:$missing (a peer holding one would then share target/ with this build)"
fi

echo ""
echo "engine build spares other builds: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
