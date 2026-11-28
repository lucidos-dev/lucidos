#!/usr/bin/env bash
# Check compilation target sizes and the ceiling ratchet, which allows 10% headroom.
set -euo pipefail
exec python3 "$(dirname "$0")/lib/crate_guardrails.py" size "$@"
