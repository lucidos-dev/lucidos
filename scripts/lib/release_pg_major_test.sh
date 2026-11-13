#!/usr/bin/env bash
# Tests for scripts/lib/release_pg_major.sh: no bundle ships another Postgres
# major than the one installed data dirs are on (ADR 0626).
# Offline: it runs each build script's --check, which builds nothing.
# Run: ./scripts/lib/release_pg_major_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=scripts/lib/release_pg_major.sh
source "$SCRIPT_DIR/release_pg_major.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

echo "test: the bundled Postgres major is pinned"
# Pins a contract: every installed pgdata is on this major. Changing it locks
# every user out of every workspace until a cross-major path exists.
if [ "$BUNDLED_PG_MAJOR" = "18" ]; then pass "BUNDLED_PG_MAJOR is 18"
else fail "BUNDLED_PG_MAJOR is '$BUNDLED_PG_MAJOR'. No cross-major path exists, so it must stay 18 (ADR 0626)"; fi

echo ""
echo "test: release_pg_major_guard accepts only the pinned major"
for v in "$BUNDLED_PG_MAJOR.4.0" "$BUNDLED_PG_MAJOR.9.1"; do
    if release_pg_major_guard "$v" 2>/dev/null; then pass "$v accepted"; else fail "$v refused"; fi
done
for v in "$((BUNDLED_PG_MAJOR + 1)).0.0" "$((BUNDLED_PG_MAJOR - 1)).6.0" "${BUNDLED_PG_MAJOR}0.1.0" "" "x"; do
    if release_pg_major_guard "$v" 2>/dev/null; then fail "'$v' accepted"; else pass "'$v' refused"; fi
done
out="$(release_pg_major_guard "$((BUNDLED_PG_MAJOR + 1)).0.0" 2>&1)"
if echo "$out" | grep -q "cross-major"; then pass "the refusal says why"; else fail "refusal lacks the reason: $out"; fi

echo ""
echo "test: both build scripts default to the pinned major"
for script in build-headless.sh build-dmg.sh; do
    # shellcheck disable=SC2016 # the pattern matches the literal ${PG_VERSION:-...} text
    default="$(sed -n 's/^PG_VERSION="${PG_VERSION:-\([0-9.]*\)}".*/\1/p' "$SCRIPTS/$script")"
    if release_pg_major_guard "$default" 2>/dev/null; then pass "$script default $default"
    else fail "$script default '$default' is not Postgres $BUNDLED_PG_MAJOR"; fi
done

echo ""
echo "test: both build scripts refuse another major before building"
for script in build-headless.sh build-dmg.sh; do
    out="$(PG_VERSION="$((BUNDLED_PG_MAJOR + 1)).0.0" "$SCRIPTS/$script" --check 2>&1)"; rc=$?
    if [ $rc -ne 0 ] && echo "$out" | grep -q "cross-major"; then pass "$script refuses PG_VERSION=$((BUNDLED_PG_MAJOR + 1)).0.0"
    else fail "$script did not refuse (rc=$rc): $out"; fi
done

echo ""
echo "release_pg_major_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
