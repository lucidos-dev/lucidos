#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/dev_prerequisites.sh
source "$SCRIPT_DIR/lib/dev_prerequisites.sh"

case "${1:-}" in
    --all) if [[ $# == 1 ]]; then suites=lint-static,ts,engine; fi ;;
    --for) if [[ $# == 2 && -n $2 ]]; then suites=$2; fi ;;
esac
if [[ -z ${suites:-} ]]; then
    echo "Usage: $0 --all | --for <comma-separated-suites>" >&2
    exit 2
fi
dev_prerequisites "$(dirname "$SCRIPT_DIR")" "$suites"
