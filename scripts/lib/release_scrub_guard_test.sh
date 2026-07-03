#!/bin/bash
# Tests for scripts/lib/private_data_patterns.sh — the deterministic private-data
# guard that scripts/release-to-lucidos.sh runs against the release tree before
# the irreversible public push.
#
# Hermetic: builds a throwaway git repo, writes a tree, and asserts
# private_data_grep_tree() flags planted leaks (known tokens AND novel
# same-shape slips) while passing legitimate attribution and the approved
# generic placeholders.
#
# Run: ./scripts/lib/release_scrub_guard_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/private_data_patterns.sh
source "$SCRIPT_DIR/private_data_patterns.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

REPO="$(mktemp -d)"
trap 'rm -rf "$REPO"' EXIT
git -C "$REPO" init -q
git -C "$REPO" config user.email "t@t"
git -C "$REPO" config user.name "t"

write() { # <relpath> <content>
  mkdir -p "$REPO/$(dirname "$1")"
  printf '%s\n' "$2" > "$REPO/$1"
}

# ── Legitimate — must NOT be flagged ────────────────────────────────────
write LICENSE 'Copyright (c) 2026 Kenneth Tiller'                    # attribution (space form)
write GOVERNANCE.md 'Akram has contributed to the project.'         # contributor credit (allowlisted file)
write clean.md 'App habit-tracker, repo example-repo, "My MacBook", path /Users/me/x, /Users/.../foo'

# ── Planted leaks — must be flagged ─────────────────────────────────────
write leak_token.md 'see artifacts/projects/pappa/folgebrev-fullmakt.pdf in m10s-green'  # denylist
write leak_app.ts "const id = 'momentum-autoresearch';"                                  # denylist (work app)
write leak_home.rs 'let p = "/Users/bobsmith/secret";'                                    # heuristic: real home dir
write leak_device.ts "const label = \"Alice's iPhone\";"                                 # heuristic: possessive device
write leak_maintainer.rs 'let p = "/Users/kenneth/ws"; // kenneth.tiller@example'        # heuristic: kenneth path/email

git -C "$REPO" add -A
TREE="$(git -C "$REPO" write-tree)"
HITS="$(private_data_grep_tree "$TREE" "$REPO")"

flagged() { printf '%s\n' "$HITS" | grep -q "$1"; }

echo "test: planted leaks are flagged"
for f in leak_token.md leak_app.ts leak_home.rs leak_device.ts leak_maintainer.rs; do
  if flagged "$f"; then pass "flagged $f"; else fail "did NOT flag $f"; fi
done

echo "test: legitimate attribution + placeholders pass"
for f in LICENSE GOVERNANCE.md clean.md; do
  if flagged "$f"; then
    fail "wrongly flagged $f → $(printf '%s\n' "$HITS" | grep "$f")"
  else
    pass "passed $f"
  fi
done

echo ""
echo "  ($PASS passed, $FAIL failed)"
[ "$FAIL" -eq 0 ]
