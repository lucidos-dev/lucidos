#!/usr/bin/env bash
# Register the lucidos merge drivers in this repository's shared git config.
# Every worktree reads that config, so one run covers the agents' merges, the
# resolvers' and the engine's Apply catch-up. `.gitattributes` picks the paths.
#
# Idempotent. /harden runs it before merging main. When it refuses, merges stay
# plain text, as before. Why: ADR 0460.
#
# Usage: ./scripts/merge-drivers.sh [path-to-lucidos-cli]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/merge_drivers.sh
source "$SCRIPT_DIR/lib/merge_drivers.sh"

if ! cli=$(merge_driver_cli "${1:-}"); then
  echo "merge-drivers: not registered; merges stay plain text" >&2
  exit 1
fi
for kind in "${MERGE_DRIVER_KINDS[@]}"; do
  git config --local "merge.lucidos-$kind.name" "lucidos $kind merge driver"
  git config --local "merge.lucidos-$kind.driver" "$(merge_driver_command "$cli" "$kind")"
done
echo "merge-drivers: registered ${MERGE_DRIVER_KINDS[*]} with $cli"
