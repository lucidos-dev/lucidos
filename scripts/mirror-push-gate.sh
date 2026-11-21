#!/usr/bin/env bash
# mirror-push-gate.sh: install the public mirror's refusing push URL in this
# repository's shared git config (ADR 0462). Idempotent, and a no-op where the
# mirror remote or the private release lib is absent.
#
# release_tree_push installs the same gate before every push, so this script
# only closes the window before a machine's first gated push. /harden Phase 4.5
# runs it.
#
# Run: ./scripts/mirror-push-gate.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"

# A public-mirror checkout ships no release_tree.sh, and has no mirror to guard.
[ -f "$SCRIPT_DIR/lib/release_tree.sh" ] || exit 0

# shellcheck source=scripts/lib/release_tree.sh
source "$SCRIPT_DIR/lib/release_tree.sh"

release_mirror_gate "$REPO_ROOT" "$RELEASE_MIRROR_REMOTE"
