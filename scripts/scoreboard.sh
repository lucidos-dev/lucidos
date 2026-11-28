#!/usr/bin/env bash
# scoreboard.sh: the delivery scoreboard. Lead time and stages per change, unit
# costs, rework, and where a /harden round's time goes
# (docs/plans/2026-11-19-scoreboard-2-stage-rules-and-script.md).
#
# Read-only. It reads the complete history of each event it uses, page by
# page. The database URL comes from LUCIDOS_DATABASE_URL and reaches psql as
# PG* variables, never a command line. The stage rules live in
# scripts/scoreboard/stage-rules.mjs. See docs/scoreboard.md for lab runs.
#
#   ./scripts/scoreboard.sh              # changes that moved in the last 7 days
#   ./scripts/scoreboard.sh --days 30
#   ./scripts/scoreboard.sh --json
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/e2e_github.sh
source "$SCRIPT_DIR/lib/e2e_github.sh"
# shellcheck source=lib/harden_suites.sh
source "$SCRIPT_DIR/lib/harden_suites.sh"

for tool in node psql; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "ERROR: scoreboard.sh needs $tool on PATH." >&2
        exit 2
    fi
done

# Older rounds recorded the cargo lint suite as lint and shell-lint.
cargo_suites="$HS_CARGO_SUITES lint shell-lint"
slot_labels() {
    local suite label
    for suite in $cargo_suites; do
        while IFS= read -r label; do
            printf '%s\t%s\n' "$suite" "$label"
        done < <(hs_slot_labels "$suite")
    done
}
exec node "$SCRIPT_DIR/scoreboard/report.mjs" \
    --timings-log "$(e2e_github_timings_log)" \
    --cargo-suites "$cargo_suites" \
    --slot-labels "$(slot_labels)" \
    "$@"
