#!/usr/bin/env bash
# Tests for scripts/lib/resource_contract.sh: the staged-resource contract and
# the check that enforces it. Pure shell over the real tree plus doctored copies
# of it, no network/cargo/npm/tauri, so the whole suite runs offline.
# Run: ./scripts/lib/resource_contract_test.sh
#
# THE POINT OF THIS SUITE is the red half. The bug it guards was a check that
# could not fail: two literals in one file, compared with each other. So most of
# what follows removes a resource from ONE of the three sources and asserts the
# check goes red naming it. A green-only suite here would reproduce the bug.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
DESKTOP_RS="$PROJECT_DIR/crates/lucidos-app/src/desktop.rs"
# shellcheck source=scripts/lib/resource_contract.sh
source "$SCRIPT_DIR/resource_contract.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# A throwaway copy of scripts/lib plus the one Rust file the check reads, so a
# case can doctor any of the three sources without touching the real tree. Prints
# the scratch root; the copied lib is at <root>/scripts/lib.
scratch_tree() {
    local root; root="$(mktemp -d)"
    mkdir -p "$root/scripts" "$root/crates/lucidos-app/src"
    cp -R "$SCRIPT_DIR" "$root/scripts/lib"
    cp "$DESKTOP_RS" "$root/crates/lucidos-app/src/desktop.rs"
    printf '%s' "$root"
}

# Run resource_contract_check out of a scratch tree, in a FRESH bash rather than
# a subshell. A subshell inherits this one's function definitions, so the lib's
# "already sourced?" guard would skip the doctored service.sh and the case would
# silently test the real one.
scratch_check() {
    local root="$1"
    bash -c "source '$root/scripts/lib/resource_contract.sh'; \
             resource_contract_check '$root/crates/lucidos-app/src/desktop.rs'" 2>&1
}

# ── the real tree ────────────────────────────────────────────────────────────

echo "test: the contract holds for the tree as committed"
out="$(resource_contract_check "$DESKTOP_RS" 2>&1)"; rc=$?
if [ $rc -eq 0 ]; then pass "check exits 0"; else fail "check exited $rc: $out"; fi
for name in lucidos-engine lucidos-gateway frontend postgres sdk system-knowhow; do
    if echo "$out" | grep -q "$name"; then pass "names $name"; else fail "missing $name from: $out"; fi
done
# The `lucidos` CLI is a distinct resource, and a substring match cannot assert
# it: a bare `lucidos` passes on `lucidos-engine` alone. Match the standalone
# space-delimited token, as build_headless_test.sh does for the same resource.
if echo "$out" | grep -qE '(^| )lucidos( |$)'; then pass "names the lucidos CLI"; else fail "missing the lucidos CLI from: $out"; fi

echo ""
echo "test: the three sources agree, and each names all seven"
if [ "$(resource_contract_names | wc -l | tr -d ' ')" = "7" ]; then pass "seven staged resources"; else fail "expected seven staged resources"; fi
if [ "$(resource_contract_runtime_required | wc -l | tr -d ' ')" = "7" ]; then pass "the headless launcher reaches seven"; else fail "headless launcher set is not seven: $(resource_contract_runtime_required | tr '\n' ' ')"; fi
if [ "$(resource_contract_desktop_names "$DESKTOP_RS" | wc -l | tr -d ' ')" = "7" ]; then pass "the packaged launcher reaches seven"; else fail "desktop.rs set is not seven: $(resource_contract_desktop_names "$DESKTOP_RS" | tr '\n' ' ')"; fi

echo ""
echo "test: the Tauri resource map is derived, not restated"
map="$(resource_contract_tauri_map_json)"
for name in $(resource_contract_names); do
    case "$map" in
        *"\"bundle-resources/$name\":\"$name\""*) pass "map carries $name" ;;
        *) fail "map is missing $name: $map" ;;
    esac
done
# One member per name, so the map cannot silently carry an eighth entry.
if [ "$(printf '%s\n' "$map" | tr ',' '\n' | wc -l | tr -d ' ')" = "7" ]; then pass "map has exactly seven members"; else fail "map member count is wrong: $map"; fi

echo ""
echo "test: every bundled executable is a staged resource"
staged="$(resource_contract_names)"
for exe in $(resource_contract_executables); do
    if printf '%s\n' "$staged" | grep -qx "$exe"; then pass "$exe is staged"; else fail "$exe is signed but not staged"; fi
done

# ── the red proofs: remove a resource from ONE source ────────────────────────

echo ""
echo "test: deleting a resource from the SHARED LIST turns the check red"
# The finding, exactly: system-knowhow dropped from what gets staged. The two
# launchers still reach for it, so nothing has to be edited in step with it.
ROOT="$(scratch_tree)"
perl -0pi -e 's/^system-knowhow\n//m' "$ROOT/scripts/lib/resource_contract.sh"
out="$(scratch_check "$ROOT")"; rc=$?
if [ $rc -ne 0 ]; then pass "a shortened list exits non-zero"; else fail "a list without system-knowhow must not pass: $out"; fi
if echo "$out" | grep -q "system-knowhow"; then pass "the refusal names system-knowhow"; else fail "the refusal must name the resource: $out"; fi
if echo "$out" | grep -q "nothing stages it"; then pass "and says nothing stages it"; else fail "expected a 'nothing stages it' refusal: $out"; fi
rm -rf "$ROOT"

echo ""
echo "test: every resource is individually load-bearing in the list"
# Not just system-knowhow. Whichever one is dropped, at least one launcher still
# reaches for it, so the check has to notice.
for name in $(resource_contract_names); do
    ROOT="$(scratch_tree)"
    perl -0pi -e "s/^\Q$name\E\n//m" "$ROOT/scripts/lib/resource_contract.sh"
    out="$(scratch_check "$ROOT")"; rc=$?
    if [ $rc -ne 0 ] && echo "$out" | grep -q "$name"; then pass "dropping $name is caught"; else fail "dropping $name went unnoticed (rc=$rc): $out"; fi
    rm -rf "$ROOT"
done

echo ""
echo "test: removing a resource from the HEADLESS launcher turns the check red"
# The other direction, and the one that proves the check is not self-referential:
# the list is untouched, and service.sh (which the build scripts do not own) stops
# naming a resource.
ROOT="$(scratch_tree)"
perl -0pi -e "s/^[ \t]*printf 'LUCIDOS_SYSTEM_KNOWHOW_DIR.*\n//m" "$ROOT/scripts/lib/service.sh"
out="$(scratch_check "$ROOT")"; rc=$?
if [ $rc -ne 0 ]; then pass "a service env missing system-knowhow exits non-zero"; else fail "the check must read service.sh, not just its own list: $out"; fi
if echo "$out" | grep -q "does not use it"; then pass "the refusal says the launcher stopped using it"; else fail "expected a 'does not use it' refusal: $out"; fi
rm -rf "$ROOT"

echo ""
echo "test: removing a resource from the PACKAGED launcher turns the check red"
ROOT="$(scratch_tree)"
perl -0pi -e 's/^const SYSTEM_KNOWHOW_RESOURCE_NAME.*\n//m' "$ROOT/crates/lucidos-app/src/desktop.rs"
out="$(scratch_check "$ROOT")"; rc=$?
if [ $rc -ne 0 ]; then pass "a desktop.rs missing the constant exits non-zero"; else fail "the check must read desktop.rs: $out"; fi
if echo "$out" | grep -q "desktop.rs"; then pass "the refusal names the packaged launcher"; else fail "expected the refusal to name desktop.rs: $out"; fi
rm -rf "$ROOT"

echo ""
echo "test: an unreadable packaged launcher is refused, never waved through"
out="$(resource_contract_check /no/such/desktop.rs 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "not found"; then pass "a missing desktop.rs is a refusal"; else fail "expected a missing-source refusal (rc=$rc): $out"; fi

# ── build-headless.sh --check can actually fail ──────────────────────────────

echo ""
echo "test: build-headless.sh --check goes red on a doctored tree"
# Its --check used to be a printf and an exit 0. Run the REAL script out of a
# scratch tree so this asserts the wiring, not just the lib.
ROOT="$(scratch_tree)"
cp "$PROJECT_DIR/scripts/build-headless.sh" "$ROOT/scripts/build-headless.sh"
cp "$PROJECT_DIR/RELEASE" "$ROOT/RELEASE" 2>/dev/null || true
out="$("$ROOT/scripts/build-headless.sh" --check 2>&1)"; rc=$?
if [ $rc -eq 0 ]; then pass "an intact scratch tree still passes"; else fail "scratch tree should pass before doctoring (rc=$rc): $out"; fi
perl -0pi -e 's/^system-knowhow\n//m' "$ROOT/scripts/lib/resource_contract.sh"
out="$("$ROOT/scripts/build-headless.sh" --check 2>&1)"; rc=$?
if [ $rc -ne 0 ]; then pass "--check exits non-zero once system-knowhow is dropped"; else fail "--check must be able to fail: $out"; fi
if echo "$out" | grep -q "system-knowhow"; then pass "--check names the missing resource"; else fail "--check must name it: $out"; fi
rm -rf "$ROOT"

# ── the staged tree, not the declarations ────────────────────────────────────

echo ""
echo "test: resource_contract_assert_staged reads the tree that was written"
STAGE="$(mktemp -d)/stage"; mkdir -p "$STAGE"
for name in $(resource_contract_names); do : > "$STAGE/$name"; done
if resource_contract_assert_staged "$STAGE" 2>/dev/null; then pass "a complete stage passes"; else fail "a complete stage must pass"; fi
rm -f "$STAGE/system-knowhow"
out="$(resource_contract_assert_staged "$STAGE" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "missing 'system-knowhow'"; then pass "a stage missing a resource is caught"; else fail "expected a missing-resource refusal (rc=$rc): $out"; fi
: > "$STAGE/system-knowhow"; : > "$STAGE/leftover"
out="$(resource_contract_assert_staged "$STAGE" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "leftover"; then pass "an unexpected entry is caught"; else fail "expected an extra-entry refusal (rc=$rc): $out"; fi
out="$(resource_contract_assert_staged /no/such/stage 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "not found"; then pass "an absent stage is a refusal"; else fail "expected an absent-stage refusal (rc=$rc): $out"; fi
rm -rf "$STAGE"

echo ""
echo "resource_contract: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
