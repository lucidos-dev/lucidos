#!/usr/bin/env bash
#
# eval-comments.sh - the comment-ablation eval: do comments that justify a
# shortcut make coding agents pick worse fixes?
#
#   ./scripts/eval-comments/eval-comments.sh prepare --verify
#   ./scripts/eval-comments/eval-comments.sh run --arms A,C --runs 2
#   ./scripts/eval-comments/eval-comments.sh report
#   ./scripts/eval-comments/eval-comments.sh status
#
# Arms: A leaves the code as is, B strips every comment, C strips only the
# comment blocks that justify a limitation. Tasks live in tasks/<id>/. The
# harness docstring (harness.py) covers isolation, layout and grading.
#
# THIS SPENDS MONEY. Every run is a headless Claude Code session, and every
# grade adds two judge calls. Nothing runs this from `make test`, `/harden` or
# a workflow.
#
# Models run on Vertex with the machine's gcloud application-default
# credentials, the same path a plain `claude` uses. Export
# ANTHROPIC_VERTEX_PROJECT_ID (or PROJECT_ID) first. The region defaults to
# europe-west1; COMMENT_EVAL_CLOUD_ML_REGION overrides it.
#
#   COMMENT_EVAL_ROOT       transcripts, diffs, grades (~/.lucidos/data/comment-ablation)
#   COMMENT_EVAL_SANDBOXES  where the agents work (~/.lucidos/data/sandboxes)
#   COMMENT_EVAL_CLAUDE     the claude binary, when `claude` is a shell wrapper

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ $# -eq 0 ]; then
    echo "usage: $0 <prepare|run|grade|report|status> [args...]" >&2
    exit 1
fi

exec python3 "$SCRIPT_DIR/harness.py" "$@"
