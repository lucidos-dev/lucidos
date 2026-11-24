#!/usr/bin/env bash
set -euo pipefail
exec python3 "$(dirname "$0")/crate_guardrails_test.py" GraphTests
