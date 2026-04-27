#!/bin/bash
# Pre-push hook: block git push unless `lucidos hardened query` reports FRESH
# (i.e. /harden has run for the current HEAD). Tests-ran marker is still a
# filesystem flag at ~/.lucidos/harden-markers/lucidos-tests-<repo-hash>; the
# user touches it by hand after `cargo test` / `npm test` succeed.

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

# Only care about git push commands
echo "$COMMAND" | grep -qE '^\s*git\s+push' || exit 0

REPO_ROOT=$(git -C "$CLAUDE_PROJECT_DIR" rev-parse --show-toplevel 2>/dev/null)
[ -z "$REPO_ROOT" ] && exit 0

# Find what would be pushed (commits ahead of upstream)
UNPUSHED=$(git -C "$REPO_ROOT" log @{u}..HEAD --oneline 2>/dev/null)
if [ -z "$UNPUSHED" ]; then
  # Nothing to push — allow it (push will be a no-op anyway)
  exit 0
fi

REPO_HASH=$(echo -n "$REPO_ROOT" | shasum -a 256 | cut -d' ' -f1)
TESTS_MARKER="$HOME/.lucidos/harden-markers/lucidos-tests-$REPO_HASH"
COMMIT_COUNT=$(echo "$UNPUSHED" | wc -l | tr -d ' ')

STATE=$(cd "$REPO_ROOT" && lucidos hardened query 2>/dev/null)

case "$STATE" in
  FRESH)
    if [ ! -f "$TESTS_MARKER" ]; then
      CHANGED=$(git -C "$REPO_ROOT" diff --name-only @{u}..HEAD 2>/dev/null)
      NEED_RUST=false
      NEED_TS=false
      echo "$CHANGED" | grep -qE '\.rs$' && NEED_RUST=true
      echo "$CHANGED" | grep -qE '\.(ts|tsx)$' && NEED_TS=true

      if $NEED_RUST || $NEED_TS; then
        CMDS=""
        $NEED_RUST && CMDS="cargo test -p lucidos-engine"
        $NEED_TS && CMDS="${CMDS:+$CMDS && }cd crates/lucidos-app && npm test"
        cat >&2 << TESTEOF
BLOCKED: Run tests before pushing.

/harden is done, but tests haven't been verified after review changes.

Run: $CMDS
After tests pass, write the marker:
  touch $TESTS_MARKER
Then retry the push.
TESTEOF
        exit 2
      fi
    fi

    # Clear tests marker so the next push re-verifies.
    rm -f "$TESTS_MARKER"
    exit 0
    ;;
  STALE)
    rm -f "$TESTS_MARKER"
    cat >&2 << EOF
BLOCKED: /harden marker is stale (HEAD moved since it was run).

Run /harden again to review the current changes.
EOF
    exit 2
    ;;
  *)  # MISSING (or empty if the engine wasn't reachable)
    cat >&2 << EOF
BLOCKED: Run /harden before pushing.

$COMMIT_COUNT unpushed commit(s):
$UNPUSHED

You MUST invoke the /harden skill to review these changes.
/harden writes the marker automatically when it completes (via mark-harden.sh).
Then retry the push.
EOF
    exit 2
    ;;
esac
