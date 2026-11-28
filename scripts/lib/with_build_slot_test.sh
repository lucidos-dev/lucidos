#!/bin/bash
# Tests for scripts/with-build-slot.sh. The memory gate (ADR 0351): no build
# starts while the gate refuses, and a build that starts carries the pass to
# the builds nested under it. The stale incremental prune runs before the
# build and never fails or delays it.
#
# Hermetic: the script is copied into a sandbox beside a stub gate, and PATH
# holds no `lucidos`, so no real slot is taken and no host is read. `lsof` is
# a stub that finds no lock holder.
#
# Run: ./scripts/lib/with_build_slot_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REAL="$SCRIPT_DIR/../with-build-slot.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
expect_eq() { # <name> <expected> <actual>
    if [ "$2" = "$3" ]; then pass "$1"; else fail "$1: expected [$2], got [$3]"; fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$TMP/scripts/lib" "$TMP/bin"
cp "$REAL" "$TMP/scripts/with-build-slot.sh"
cp "$SCRIPT_DIR/stale_incremental.sh" "$SCRIPT_DIR/cargo_build_lock.sh" "$SCRIPT_DIR/flock.sh" "$TMP/scripts/lib/"
printf '#!/bin/bash\nexit 1\n' > "$TMP/bin/lsof"
chmod +x "$TMP/bin/lsof"
# The stub gate exits with the code in gate.rc and records its label.
cat > "$TMP/scripts/build-memory-gate.sh" << 'EOF'
#!/bin/bash
d="$(dirname "$0")/.."
printf '%s\n' "$*" >> "$d/gate.calls"
exit "$(cat "$d/gate.rc" 2>/dev/null || echo 0)"
EOF
chmod +x "$TMP/scripts/build-memory-gate.sh"
# The "build": records that it ran and the pass it inherited.
cat > "$TMP/bin/fake-build" << 'EOF'
#!/bin/bash
echo "${LUCIDOS_BUILD_MEMORY_GATE_PASSED_AT:-none}" > "$(dirname "$0")/../build.ran"
exit "$(cat "$(dirname "$0")/../build.rc" 2>/dev/null || echo 0)"
EOF
chmod +x "$TMP/bin/fake-build"

# No `lucidos` anywhere on PATH, so the broker resolves to nothing and the
# script degrades to running the command itself.
SAFE_PATH="$TMP/bin:/usr/bin:/bin"
run_slot() { # <label>
    (cd "$TMP" && env -i HOME="$TMP" PATH="$SAFE_PATH" \
        LUCIDOS_SLOT_RECHECK_MEMORY="${LUCIDOS_SLOT_RECHECK_MEMORY:-}" \
        bash "$TMP/scripts/with-build-slot.sh" --label "$1" -- fake-build)
}

echo "a refused gate starts no build"
rm -f "$TMP/build.ran" "$TMP/gate.calls"
echo 72 > "$TMP/gate.rc"
run_slot "engine build (release)" > "$TMP/out" 2>&1
expect_eq "exit is the gate's 72" 72 "$?"
if [ -f "$TMP/build.ran" ]; then fail "the build ran on a refused gate"; else pass "the build never ran"; fi
expect_eq "the gate was asked about this build" "--label engine build (release)" "$(cat "$TMP/gate.calls")"

echo "a passed gate starts the build and hands the pass down"
rm -f "$TMP/build.ran" "$TMP/gate.calls"
echo 0 > "$TMP/gate.rc"
run_slot "make lint" > "$TMP/out" 2>&1
expect_eq "exit 0" 0 "$?"
if [ -f "$TMP/build.ran" ]; then pass "the build ran"; else fail "the build did not run"; fi
case "$(cat "$TMP/build.ran" 2>/dev/null)" in
    '' | none | *[!0-9]*) fail "the build inherited no pass timestamp" ;;
    *) pass "the build inherited the pass timestamp" ;;
esac

echo "an unlabelled build is named by its command"
rm -f "$TMP/gate.calls"
(cd "$TMP" && env -i HOME="$TMP" PATH="$SAFE_PATH" \
    bash "$TMP/scripts/with-build-slot.sh" -- fake-build) > "$TMP/out" 2>&1
expect_eq "the label is the command" "--label fake-build" "$(cat "$TMP/gate.calls")"

# A crate dir untouched for ten days, in the sandbox's own target dir.
seed_stale_crate_dir() {
    rm -rf "$TMP/target" "$TMP/build.ran"
    mkdir -p "$TMP/target/debug/incremental/api-28e9f3ts2tj7p"
    touch -t "$(date -v-10d +%Y%m%d%H%M 2>/dev/null || date -d '10 days ago' +%Y%m%d%H%M)" \
        "$TMP/target/debug/incremental/api-28e9f3ts2tj7p"
}

echo "a stale incremental dir goes before the build, and the build's exit passes through"
seed_stale_crate_dir
echo 3 > "$TMP/build.rc"
run_slot "make lint" > "$TMP/out" 2>&1
expect_eq "exit is the build's 3" 3 "$?"
rm -f "$TMP/build.rc"
if [ -f "$TMP/build.ran" ]; then pass "the build ran"; else fail "the build did not run"; fi
for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -e "$TMP/target/debug/incremental/api-28e9f3ts2tj7p" ] || [ -n "$(find "$TMP/target" -maxdepth 1 -name ".stale-incremental*" -type d)" ] || break
    sleep 0.3
done
if [ -e "$TMP/target/debug/incremental/api-28e9f3ts2tj7p" ] || [ -n "$(find "$TMP/target" -maxdepth 1 -name ".stale-incremental*" -type d)" ]; then
    fail "the stale dir is still there"
else
    pass "the stale dir was moved aside and deleted"
fi

echo "a prune that fails or cannot load still lets the build run"
for broken in 'prune_stale_incremental() { return 7; }' 'prune_stale_incremental( {'; do
    seed_stale_crate_dir
    printf '%s\n' "$broken" > "$TMP/scripts/lib/stale_incremental.sh"
    run_slot "make lint" > "$TMP/out" 2>&1
    expect_eq "exit 0 with [$broken]" 0 "$?"
    if [ -f "$TMP/build.ran" ]; then pass "the build ran"; else fail "the build did not run"; fi
done

cat > "$TMP/bin/lucidos" <<'EOF'
#!/bin/bash
[ "$1" = build-slot ] || exit 2
if [ "$2" = --help ]; then
    [ -z "${NEW_BROKER:-}" ] || echo 'Script protocol: --lease'
    exit 0
fi
if [ "$4" = --lease ]; then
    echo 'granted 0 2 10'
    read -r _
    exit 0
fi
while [ "$1" != -- ]; do shift; done
shift
touch "$(dirname "$0")/../broker-admitted"
exec "$@"
EOF
chmod +x "$TMP/bin/lucidos"

# shellcheck source=scripts/lib/build_slot_lease.sh
source "$SCRIPT_DIR/build_slot_lease.sh"
leased_body() {
    "$PROJECT_DIR/scripts/with-build-slot.sh" -- fake-build
}

test_an_old_broker_falls_back_to_per_step_slots() {
    (cd "$TMP" && PATH="$SAFE_PATH" PROJECT_DIR="$TMP" build_slot_leased engine leased_body)
    expect_eq "old broker ran the build" 0 "$?"
}
test_no_broker_runs_the_body_unleased() {
    mv "$TMP/bin/lucidos" "$TMP/bin/lucidos.saved"
    (cd "$TMP" && PATH="$SAFE_PATH" PROJECT_DIR="$TMP" build_slot_leased engine leased_body)
    expect_eq "missing broker ran the build" 0 "$?"
    mv "$TMP/bin/lucidos.saved" "$TMP/bin/lucidos"
}
test_lease_runs_the_memory_gate_once() {
    rm -f "$TMP/gate.calls" "$TMP/build.ran"
    (cd "$TMP" && PATH="$SAFE_PATH" PROJECT_DIR="$TMP" NEW_BROKER=1 \
        build_slot_leased engine leased_body)
    expect_eq "leased build ran" 0 "$?"
    expect_eq "gate ran once before acquisition" "--label engine" "$(cat "$TMP/gate.calls")"
    case "$(cat "$TMP/build.ran" 2>/dev/null)" in
        '' | none | *[!0-9]*) fail "the leased build inherited no pass timestamp" ;;
        *) pass "the leased build inherited the pass timestamp" ;;
    esac
}
rm -f "$TMP/scripts/lib/stale_incremental.sh"
unset LUCIDOS_BUILD_SLOT_HELD LUCIDOS_BUILD_SLOT_LEASE_NICE
test_an_old_broker_falls_back_to_per_step_slots
test_no_broker_runs_the_body_unleased
test_lease_runs_the_memory_gate_once

echo ""
cat > "$TMP/scripts/build-prerequisites.sh" <<'CHECK'
#!/bin/bash
d="$(dirname "$0")/.."
[ -f "$d/broker-admitted" ] || exit 1
echo "$*" > "$d/strict-memory.calls"
exit 72
CHECK
chmod +x "$TMP/scripts/build-prerequisites.sh"
rm -f "$TMP/build.ran" "$TMP/broker-admitted"
LUCIDOS_SLOT_RECHECK_MEMORY=1 run_slot "strict memory recheck"
expect_eq "strict memory refusal preserves exit 72" 72 "$?"
expect_eq "strict memory is probed without a prior receipt" --memory "$(cat "$TMP/strict-memory.calls")"
expect_eq "strict memory refusal prevents execution" missing "$(cat "$TMP/build.ran" 2>/dev/null || echo missing)"

echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
