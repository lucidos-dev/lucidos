#!/bin/bash
# Tests for scripts/lib/pg_secret.sh, the dev shared cluster's password.
#
# Hermetic: every case runs under a throwaway HOME, so the machine's own
# ~/.lucidos is never read or written.
#
# Run: ./scripts/lib/pg_secret_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/pg_secret.sh
source "$SCRIPT_DIR/pg_secret.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

mode_of() { stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1"; }

echo "a first call mints a 64-hex secret, readable by its owner alone"
export HOME="$TMP/fresh"
first="$(shared_pg_password)"
if [[ "$first" =~ ^[0-9a-f]{$((TOKEN_BYTES * 2))}$ ]]; then pass "TOKEN_BYTES of lowercase hex"; else fail "not TOKEN_BYTES of hex: '$first'"; fi
file="$HOME/.lucidos/$SHARED_PG_PASSWORD_FILE"
if [ "$(mode_of "$file")" = "600" ]; then pass "mode 0600"; else fail "mode $(mode_of "$file")"; fi
if [ "$(mode_of "$HOME/.lucidos")" = "700" ]; then pass "the Lucidos home it made is 0700"; else fail "the Lucidos home is mode $(mode_of "$HOME/.lucidos")"; fi
if [ "$(shared_pg_password)" = "$first" ]; then pass "a second call reads the same secret"; else fail "a second call minted again"; fi
if [ -z "$(find "$HOME/.lucidos" -name ".$SHARED_PG_PASSWORD_FILE.*.tmp")" ]; then pass "no temp file left behind"; else fail "a temp file was left behind"; fi

echo "an existing secret wins, whoever wrote it"
export HOME="$TMP/existing"
mkdir -p "$HOME/.lucidos"
printf 'from-the-gateway\n' >"$HOME/.lucidos/$SHARED_PG_PASSWORD_FILE"
chmod 644 "$HOME/.lucidos/$SHARED_PG_PASSWORD_FILE"
if [ "$(shared_pg_password)" = "from-the-gateway" ]; then pass "kept, trailing newline trimmed"; else fail "replaced an existing secret"; fi
if [ "$(mode_of "$HOME/.lucidos/$SHARED_PG_PASSWORD_FILE")" = "600" ]; then pass "a loose mode is tightened"; else fail "left a loose mode"; fi

echo "an empty file was never published, so a fresh secret replaces it"
export HOME="$TMP/blank"
mkdir -p "$HOME/.lucidos"
: >"$HOME/.lucidos/$SHARED_PG_PASSWORD_FILE"
blank="$(shared_pg_password)"
if [[ "$blank" =~ ^[0-9a-f]{$((TOKEN_BYTES * 2))}$ ]]; then pass "minted over the blank"; else fail "returned '$blank'"; fi
if [ "$(tr -d '[:space:]' <"$HOME/.lucidos/$SHARED_PG_PASSWORD_FILE")" = "$blank" ]; then pass "and published it"; else fail "returned a secret it never published"; fi
if [ -e "$HOME/.lucidos/.$SHARED_PG_PASSWORD_FILE.$MINT_LOCK_SUFFIX" ]; then pass "the mint lock is the gateway's lock file"; else fail "no lock file at the gateway's path"; fi

echo "an unreadable secret is an error, never minted over"
export HOME="$TMP/unreadable"
mkdir -p "$HOME/.lucidos"
printf 'kept\n' >"$HOME/.lucidos/$SHARED_PG_PASSWORD_FILE"
chmod 000 "$HOME/.lucidos/$SHARED_PG_PASSWORD_FILE"
if shared_pg_password >/dev/null 2>&1; then fail "returned a secret"; else pass "fails"; fi
chmod 600 "$HOME/.lucidos/$SHARED_PG_PASSWORD_FILE"
if [ "$(cat "$HOME/.lucidos/$SHARED_PG_PASSWORD_FILE")" = "kept" ]; then pass "the secret is untouched"; else fail "minted over it"; fi

echo "racing minters all end on one secret"
export HOME="$TMP/race"
for i in 1 2 3 4 5 6 7 8; do
    shared_pg_password >"$TMP/race-$i" &
done
wait
distinct="$(sort -u "$TMP"/race-* | wc -l | tr -d ' ')"
if [ "$distinct" = "1" ]; then pass "eight minters, one secret"; else fail "$distinct different secrets"; fi
if [ "$(cat "$TMP/race-1")" = "$(shared_pg_password)" ]; then pass "the file holds that secret"; else fail "the file holds another secret"; fi

echo ""
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ]
