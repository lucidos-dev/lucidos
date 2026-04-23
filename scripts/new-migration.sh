#!/bin/bash
# Create a new sqlx migration file with a guaranteed-unique YYYYMMDDHHMMSS prefix.
#
# Usage:
#   ./scripts/new-migration.sh add_thread_presence
#   ./scripts/new-migration.sh "user question answered unique"
#
# Why this exists: parallel CC sessions kept picking the same placeholder
# timestamp (e.g. 20260417120000) and producing migrations whose version
# prefixes collided in `_sqlx_migrations`, crashing the engine on startup.
# This script always uses the real wall-clock second AND bumps if the slot
# is already taken on disk.

set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <description>" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MIGRATIONS_DIR="$SCRIPT_DIR/../crates/cognos-engine/migrations"

description="$*"
slug="$(echo "$description" | tr '[:upper:] ' '[:lower:]_' | tr -cd 'a-z0-9_')"

ts="$(date +%Y%m%d%H%M%S)"
while ls "$MIGRATIONS_DIR"/${ts}_*.sql >/dev/null 2>&1; do
  ts=$(printf "%014d" $((10#$ts + 1)))
done

path="$MIGRATIONS_DIR/${ts}_${slug}.sql"
touch "$path"
echo "$path"
