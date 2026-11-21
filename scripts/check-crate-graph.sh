#!/usr/bin/env bash
# Inspect the workspace dependency graph and its rebuild impact.
set -euo pipefail
exec python3 "$(dirname "$0")/lib/crate_guardrails.py" graph "$@"
