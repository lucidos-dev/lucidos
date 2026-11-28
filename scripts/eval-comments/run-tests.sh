#!/usr/bin/env bash
# run-tests.sh: the comment eval harness's own tests.
#
#   ./scripts/eval-comments/run-tests.sh
#
# The tests stub every model and cargo call, so they spend nothing. The eval
# itself (eval-comments.sh) spends money and never runs from here.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

python3 "$HERE/harness_test.py"
