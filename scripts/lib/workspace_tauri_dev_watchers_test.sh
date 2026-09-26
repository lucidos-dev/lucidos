#!/bin/bash
# Tests for select_tauri_dev_watchers in scripts/lib/workspace.sh.
#
# tauri-dev.sh stops the previous window's watcher before it launches a new
# one. Its old loop matched `pgrep -f "cargo tauri dev"` and then required the
# executable to be `cargo`. Cargo execs the subcommand, so the live process is
# `cargo-tauri tauri dev ...` and the loop never matched anything.
#
# Hermetic: both seams are replaced for the whole file, so no real process is
# listed, read or signalled. An empty feed selects nothing.
#
# Run: ./scripts/lib/workspace_tauri_dev_watchers_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# shellcheck source=workspace.sh
source "$SCRIPT_DIR/workspace.sh"

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
APP_DIR="$(cd "$SANDBOX" && pwd -P)/checkout/crates/lucidos-app"
OTHER_DIR="$(cd "$SANDBOX" && pwd -P)/other-project/src-tauri"
mkdir -p "$APP_DIR" "$OTHER_DIR"

FEED=""
_tauri_dev_ps_feed() { [ -n "$FEED" ] || return 0; printf '%s\n' "$FEED"; }
_tauri_dev_cwd() {
    case "$1" in
        101|104|105|106) printf '%s\n' "$APP_DIR" ;;
        102) printf '%s\n' "$OTHER_DIR" ;;
        *) ;;
    esac
}

FEED="101 /home/user/.cargo/bin/cargo-tauri tauri dev --config {} -- --locked
102 /home/user/.cargo/bin/cargo-tauri tauri dev
103 /home/user/.local/bin/claude -p the script runs cargo tauri dev from crates/lucidos-app
104 /home/user/.cargo/bin/cargo-tauri tauri build
105 /home/user/.cargo/bin/cargo tauri dev
106 /home/user/.cargo/bin/cargo-tauri tauri dev"
selected="$(select_tauri_dev_watchers "$APP_DIR" | tr '\n' ' ')"

echo "test: a watcher running from the app dir is selected"
if [ "$selected" = "101 106 " ]; then
    pass "selected exactly 101 and 106"
else
    fail "expected '101 106 ', got '$selected'"
fi

echo "test: a tauri dev from another project is not selected"
case " $selected" in *" 102 "*) fail "selected 102" ;; *) pass "102 skipped" ;; esac

echo "test: a process that only mentions the phrase is not selected"
case " $selected" in *" 103 "*) fail "selected 103" ;; *) pass "103 skipped" ;; esac

echo "test: another tauri subcommand, or a plain cargo argv[0], is not selected"
case " $selected" in
    *" 104 "*|*" 105 "*) fail "selected 104 or 105" ;;
    *) pass "104 and 105 skipped" ;;
esac

echo "test: an empty feed selects nothing"
FEED=""
selected="$(select_tauri_dev_watchers "$APP_DIR")"
if [ -z "$selected" ]; then pass "nothing selected"; else fail "selected '$selected'"; fi

echo "test: a missing app dir selects nothing and does not end a set -e caller"
FEED="101 /home/user/.cargo/bin/cargo-tauri tauri dev"
out="$(
    set -e
    selected="$(select_tauri_dev_watchers "$SANDBOX/missing")"
    printf 'reached [%s]\n' "$selected"
)"
if [ "$out" = "reached []" ]; then pass "caller carried on"; else fail "output '$out'"; fi

echo "test: a non-matching last line does not end a set -e caller"
FEED="101 /home/user/.cargo/bin/cargo-tauri tauri dev
102 /home/user/.cargo/bin/cargo-tauri tauri dev"
out="$(
    set -e
    selected="$(select_tauri_dev_watchers "$APP_DIR")"
    printf 'reached [%s]\n' "$selected"
)"
if [ "$out" = "reached [101]" ]; then pass "caller carried on"; else fail "output '$out'"; fi

echo ""
echo "select_tauri_dev_watchers: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
