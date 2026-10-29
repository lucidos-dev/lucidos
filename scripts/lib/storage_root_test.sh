#!/bin/bash
# Tests for scripts/lib/storage_root.sh, the storage-root rule in shell
# (ADR 0456), and for the dev-script openers that apply it.
#
# Run: ./scripts/lib/storage_root_test.sh   (no harness; direct)
#
# Every case runs under a throwaway HOME, so nothing here touches this
# machine's own ~/.lucidos or workspaces. Nothing here starts a process.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=storage_root.sh
source "$SCRIPT_DIR/storage_root.sh"

PASS=0
FAIL=0
assert_eq() {
    if [ "$2" = "$3" ]; then
        echo "  ok:   $1"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $1 (want '$3', got '$2')"
        FAIL=$((FAIL + 1))
    fi
}

mode_of() {
    stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1"
}

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

echo "storage_root: the Lucidos home sits under HOME"
assert_eq "the home path" "$(HOME=/home/u lucidos_home_dir)" "/home/u/.lucidos"

echo "storage_root: a new root is owner-only at every created level"
(umask 000 && ensure_storage_root "$SCRATCH/a/b/root")
assert_eq "the root" "$(mode_of "$SCRATCH/a/b/root")" "700"
assert_eq "a created parent" "$(mode_of "$SCRATCH/a")" "700"

echo "storage_root: an open root converges, and its contents keep their modes"
mkdir -m 755 "$SCRATCH/open"
(umask 022 && echo x > "$SCRATCH/open/note.md")
ensure_storage_root "$SCRATCH/open"
assert_eq "the root" "$(mode_of "$SCRATCH/open")" "700"
assert_eq "a file inside" "$(mode_of "$SCRATCH/open/note.md")" "644"

echo "storage_root: a root under a file fails"
touch "$SCRATCH/file"
if ensure_storage_root "$SCRATCH/file/root" 2>/dev/null; then status=0; else status=1; fi
assert_eq "the exit status" "$status" "1"

# Each runs in its own subshell, under the throwaway HOME.
# shellcheck disable=SC2030,SC2031,SC2034 # each case scopes its environment to its own subshell
resolve_in_fake_home() (
    export HOME="$fake_home"
    # shellcheck source=workspace.sh
    source "$SCRIPT_DIR/workspace.sh"
    PROJECT_DIR=""
    WORKSPACE="$1"
    resolve_workspace >/dev/null
)
# shellcheck disable=SC2030,SC2031 # each case scopes its environment to its own subshell
gateway_data_dir_in_fake_home() (
    export HOME="$fake_home"
    unset LUCIDOS_GATEWAY_DATA
    # shellcheck source=workspace.sh
    source "$SCRIPT_DIR/workspace.sh"
    gateway_data_dir
)

echo "workspace.sh: resolve_workspace makes the workspace a storage root"
fake_home="$SCRATCH/home"
mkdir -p "$fake_home/workspaces/myws"
chmod 755 "$fake_home/workspaces/myws"
resolve_in_fake_home myws
assert_eq "an existing open workspace" "$(mode_of "$fake_home/workspaces/myws")" "700"
resolve_in_fake_home fresh
assert_eq "a new workspace" "$(mode_of "$fake_home/workspaces/fresh")" "700"

echo "workspace.sh: the dev gateway's data dir defaults under the Lucidos home"
assert_eq "the default" "$(gateway_data_dir_in_fake_home)" "$fake_home/.lucidos/gateway"

echo
echo "storage_root_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
