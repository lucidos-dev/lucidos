#!/bin/bash
# Pre-push hook: block git push until /harden has been run.
# Uses a content-verified marker (HEAD SHA) so the marker can't be
# faked with a simple `touch`. The marker must contain the current
# HEAD commit SHA — if HEAD moves after /harden, it's stale.
#
# Markers are stored in ~/.cognos/harden-markers/ keyed by repo root
# path hash so the LLM can't see or accidentally commit them. Same
# location written by .claude/hooks/mark-harden.sh.

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

# Only care about git push commands
echo "$COMMAND" | grep -qE '^\s*git\s+push' || exit 0

REPO_ROOT=$(git -C "$CLAUDE_PROJECT_DIR" rev-parse --show-toplevel 2>/dev/null)
[ -z "$REPO_ROOT" ] && exit 0

REPO_HASH=$(echo -n "$REPO_ROOT" | shasum -a 256 | cut -d' ' -f1)
MARKER="$HOME/.cognos/harden-markers/cognos-harden-$REPO_HASH"
TESTS_MARKER="$HOME/.cognos/harden-markers/cognos-tests-$REPO_HASH"

# Find what would be pushed (commits ahead of upstream)
UNPUSHED=$(git -C "$REPO_ROOT" log @{u}..HEAD --oneline 2>/dev/null)

if [ -z "$UNPUSHED" ]; then
  # Nothing to push — allow it (push will be a no-op anyway)
  exit 0
fi

EXPECTED_SHA=$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null)
COMMIT_COUNT=$(echo "$UNPUSHED" | wc -l | tr -d ' ')

if [ -f "$MARKER" ]; then
  STORED_SHA=$(head -1 "$MARKER" 2>/dev/null)
  if [ "$STORED_SHA" != "$EXPECTED_SHA" ]; then
    rm -f "$MARKER" "$TESTS_MARKER"
    cat >&2 << EOF
BLOCKED: /harden marker is stale (HEAD moved since it was run).

Run /harden again to review the current changes.
EOF
    exit 2
  fi

  # After /harden, require tests if relevant files changed
  if [ ! -f "$TESTS_MARKER" ]; then
    CHANGED=$(git -C "$REPO_ROOT" diff --name-only @{u}..HEAD 2>/dev/null)
    NEED_RUST=false
    NEED_TS=false
    echo "$CHANGED" | grep -qE '\.rs$' && NEED_RUST=true
    echo "$CHANGED" | grep -qE '\.(ts|tsx)$' && NEED_TS=true

    if $NEED_RUST || $NEED_TS; then
      CMDS=""
      $NEED_RUST && CMDS="cargo test -p cognos-engine"
      $NEED_TS && CMDS="${CMDS:+$CMDS && }cd crates/cognos-app && npm test"
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

  # All checks passed — clean up markers
  rm -f "$MARKER" "$TESTS_MARKER"
  exit 0
fi

cat >&2 << EOF
BLOCKED: Run /harden before pushing.

$COMMIT_COUNT unpushed commit(s):
$UNPUSHED

You MUST invoke the /harden skill to review these changes.
/harden writes the marker automatically when it completes (via mark-harden.sh).
Then retry the push.
EOF
exit 2
