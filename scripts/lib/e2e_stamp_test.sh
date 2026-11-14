#!/bin/bash
# Tests for scripts/lib/e2e_stamp.sh.
#
# Hermetic: it writes only under a temp sandbox and starts no process.
#
# Run: ./scripts/lib/e2e_stamp_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
assert_eq() { # <expected> <actual> <label>
    if [ "$1" = "$2" ]; then pass "$3"; else fail "$3 (expected '$1', got '$2')"; fi
}

# shellcheck source=scripts/lib/e2e_stamp.sh
source "$SCRIPT_DIR/e2e_stamp.sh"

echo "e2e_stamp_workspace_id"
assert_eq gw-slug "$(e2e_stamp_workspace_id gw-slug /home/u/workspaces/e2e-test)" "the gateway slug wins"
assert_eq e2e-test "$(e2e_stamp_workspace_id "" /home/u/workspaces/e2e-test)" "else the directory name"
assert_eq e2e-test "$(e2e_stamp_workspace_id "" /home/u/workspaces/e2e-test/)" "a trailing slash is ignored"

echo "mint_e2e_stamp_key"
KEY_FILE="$(e2e_stamp_key_path "$SANDBOX/ws")"
assert_eq "$SANDBOX/ws/.lucidos/e2e-stamp-key" "$KEY_FILE" "the key lives in the workspace's .lucidos"
if mint_e2e_stamp_key "$KEY_FILE" e2e-test; then pass "the mint succeeds"; else fail "the mint succeeds"; fi
FIRST_KEY="$(sed -n 1p "$KEY_FILE")"
if printf '%s' "$FIRST_KEY" | grep -Eq '^[0-9a-f]{64}$'; then
    pass "line one is 32 bytes of hex"
else
    fail "line one is 32 bytes of hex (got '$FIRST_KEY')"
fi
assert_eq e2e-test "$(sed -n 2p "$KEY_FILE")" "line two names the workspace"
assert_eq 2 "$(wc -l < "$KEY_FILE" | tr -d ' ')" "the file holds two lines"
MODE="$(stat -f %Lp "$KEY_FILE" 2>/dev/null || stat -c %a "$KEY_FILE")"
assert_eq 600 "$MODE" "only the owner can read the key"
mint_e2e_stamp_key "$KEY_FILE" e2e-test
if [ "$(sed -n 1p "$KEY_FILE")" != "$FIRST_KEY" ]; then
    pass "each mint is a fresh key"
else
    fail "each mint is a fresh key"
fi
LEFTOVER="$(find "$SANDBOX/ws/.lucidos" -name 'e2e-stamp-key.tmp.*' | wc -l | tr -d ' ')"
assert_eq 0 "$LEFTOVER" "no temp file is left behind"

echo ""
echo "e2e_stamp_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
