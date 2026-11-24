#!/usr/bin/env bash
# Validate workspace dependencies against crates/crate-tiers.tsv.
set -euo pipefail
exec python3 "$(dirname "$0")/lib/crate_guardrails.py" tiers "$@"
