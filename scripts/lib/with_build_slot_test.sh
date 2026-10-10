#!/bin/bash
# Tests for the build memory gate in scripts/with-build-slot.sh (ADR 0351):
# no build starts while the gate refuses, and a build that starts carries the
# pass to the builds nested under it.
#
# Hermetic: the script is copied into a sandbox beside a stub gate, and PATH
# holds no `lucidos`, so no real slot is taken and no host is read.
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

mkdir -p "$TMP/scripts" "$TMP/bin"
cp "$REAL" "$TMP/scripts/with-build-slot.sh"
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
EOF
chmod +x "$TMP/bin/fake-build"

# No `lucidos` anywhere on PATH, so the broker resolves to nothing and the
# script degrades to running the command itself.
SAFE_PATH="$TMP/bin:/usr/bin:/bin"
run_slot() { # <label>
    (cd "$TMP" && env -i HOME="$TMP" PATH="$SAFE_PATH" \
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

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
