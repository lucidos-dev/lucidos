#!/bin/bash
# Pre-push hook: block git push unless `lucidos hardened query` reports FRESH
# (i.e. /harden has run for the current HEAD). FRESH implies tests passed —
# /harden runs the test suites for the layers touched as part of its flow.

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

# Only care about git push commands
echo "$COMMAND" | grep -qE '^\s*git\s+push' || exit 0

REPO_ROOT=$(git -C "$CLAUDE_PROJECT_DIR" rev-parse --show-toplevel 2>/dev/null)
[ -z "$REPO_ROOT" ] && exit 0

# Find what would be pushed (commits ahead of upstream)
UNPUSHED=$(git -C "$REPO_ROOT" log '@{u}..HEAD' --oneline 2>/dev/null)
if [ -z "$UNPUSHED" ]; then
  # Nothing to push — allow it (push will be a no-op anyway)
  exit 0
fi

COMMIT_COUNT=$(echo "$UNPUSHED" | wc -l | tr -d ' ')
STATE=$(cd "$REPO_ROOT" && lucidos hardened query 2>/dev/null)

case "$STATE" in
  FRESH)
    exit 0
    ;;
  STALE)
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
/harden writes the marker automatically when it completes: its Phase 5 runs
the "lucidos hardened mark" subcommand. Then retry the push.
EOF
    exit 2
    ;;
esac
