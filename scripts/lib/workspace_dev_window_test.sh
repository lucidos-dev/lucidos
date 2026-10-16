#!/bin/bash
# Tests for the dev window's launch binary in scripts/lib/workspace.sh
# (ADR 0461): which processes select_dev_windows picks, when
# publish_dev_window_binary replaces the published file, and when
# build_or_find_engine builds the window.
#
# Hermetic: the ps seam is replaced for the whole file, so no real process is
# listed or signalled. The build tests stub every compile, sign and prune step,
# so no cargo runs.
#
# Run: ./scripts/lib/workspace_dev_window_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# shellcheck source=workspace.sh
source "$SCRIPT_DIR/workspace.sh"

SANDBOX="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$SANDBOX"' EXIT

FEED=""
_dev_window_ps_feed() { [ -n "$FEED" ] || return 0; printf '%s\n' "$FEED"; }

# ── select_dev_windows ─────────────────────────────────────────────────
BIN="$SANDBOX/checkout/.launch/debug/plain/lucidos-app"
SPACED="$SANDBOX/my checkout/.launch/debug/plain/lucidos-app"

FEED="101 $BIN
102 $BIN --some-flag
103 $SANDBOX/other/.launch/debug/plain/lucidos-app
104 /home/user/.local/bin/claude -p the window runs $BIN
105 $BIN.tmp.4242
106 $SPACED"
selected="$(select_dev_windows "$BIN" | tr '\n' ' ')"

echo "test: this checkout's window is selected, with or without arguments"
if [ "$selected" = "101 102 " ]; then
    pass "selected exactly 101 and 102"
else
    fail "expected '101 102 ', got '$selected'"
fi

echo "test: another checkout's window is not selected"
case " $selected" in *" 103 "*) fail "selected 103" ;; *) pass "103 skipped" ;; esac

echo "test: a process that only mentions the path is not selected"
case " $selected" in *" 104 "*) fail "selected 104" ;; *) pass "104 skipped" ;; esac

echo "test: a path that only starts with the binary's path is not selected"
case " $selected" in *" 105 "*) fail "selected 105" ;; *) pass "105 skipped" ;; esac

echo "test: a binary path holding a space is matched whole"
selected="$(select_dev_windows "$SPACED")"
if [ "$selected" = "106" ]; then pass "selected 106"; else fail "got '$selected'"; fi

echo "test: an empty feed selects nothing"
FEED=""
selected="$(select_dev_windows "$BIN")"
if [ -z "$selected" ]; then pass "nothing selected"; else fail "selected '$selected'"; fi

echo "test: a non-matching last line does not end a set -e caller"
FEED="101 $BIN
103 $SANDBOX/other/.launch/debug/plain/lucidos-app"
out="$(
    set -e
    selected="$(select_dev_windows "$BIN")"
    printf 'reached [%s]\n' "$selected"
)"
if [ "$out" = "reached [101]" ]; then pass "caller carried on"; else fail "output '$out'"; fi

# ── publish_dev_window_binary ──────────────────────────────────────────
inode() { stat -c %i "$1" 2>/dev/null || stat -f %i "$1"; }

PUB="$SANDBOX/publish"
mkdir -p "$PUB/launch"
printf 'window v1' > "$PUB/built"
publish_dev_window_binary "$PUB/built" "$PUB/launch/lucidos-app"
first="$(inode "$PUB/launch/lucidos-app")"

echo "test: a rebuild with identical bytes leaves the published file alone"
printf 'window v1' > "$PUB/built"
publish_dev_window_binary "$PUB/built" "$PUB/launch/lucidos-app"
if [ "$(inode "$PUB/launch/lucidos-app")" = "$first" ]; then
    pass "same file, so a running window stays put"
else
    fail "the file was replaced, so the next switch would relaunch the window for nothing"
fi

echo "test: a changed binary is published as a new file, stamped at publish time"
printf 'window v2' > "$PUB/built"
touch -t 200001010000 "$PUB/built"
publish_dev_window_binary "$PUB/built" "$PUB/launch/lucidos-app"
if [ "$(inode "$PUB/launch/lucidos-app")" != "$first" ] \
    && [ "$(cat "$PUB/launch/lucidos-app")" = "window v2" ]; then
    pass "new file with the new bytes"
else
    fail "expected a new file holding 'window v2'"
fi
if [ "$PUB/launch/lucidos-app" -nt "$PUB/built" ]; then
    pass "mtime is the publish, not the link"
else
    fail "the published file kept the build's mtime"
fi

# ── build_or_find_engine builds the window only where one was published ─
# Runs the real build_or_find_engine with the engine half stubbed, under
# `set -e` as web-dev.sh runs it. Prints which builds ran.
run_build() {
    local checkout="$1" window_rc="$2"
    (
        set -e
        run_engine_cargo_build() { echo engine; }
        prune_stale_debug_objects() { :; }
        publish_launch_binaries() { echo published; }
        published_build_state() { echo current; }
        sign_engine_binary() { :; }
        run_dev_window_cargo_build() {
            echo window
            printf 'window built' > "$PROJECT_DIR/target/debug/lucidos-app"
            return "$window_rc"
        }
        BUILD=1
        PROJECT_DIR="$checkout"
        build_or_find_engine
    ) 2>&1
}

echo "test: a checkout that never ran a dev window builds no window"
NO_WINDOW="$SANDBOX/no-window"
mkdir -p "$NO_WINDOW/target/debug"
out="$(run_build "$NO_WINDOW" 0)"
if printf '%s\n' "$out" | grep -qx engine && ! printf '%s\n' "$out" | grep -qx window; then
    pass "engine built, window skipped"
else
    fail "output: $out"
fi

echo "test: a checkout with a published window compiles it with the engine, before publishing"
WITH_WINDOW="$SANDBOX/with-window"
mkdir -p "$WITH_WINDOW/target/debug" "$WITH_WINDOW/.launch/debug/plain"
printf 'window old' > "$WITH_WINDOW/.launch/debug/plain/lucidos-app"
out="$(run_build "$WITH_WINDOW" 0)"
order="$(printf '%s\n' "$out" | grep -xE 'engine|window|published' | tr '\n' ' ')"
if [ "$order" = "engine window published " ] \
    && [ "$(cat "$WITH_WINDOW/.launch/debug/plain/lucidos-app")" = "window built" ]; then
    pass "engine, window, then publish, and the new window is published"
else
    fail "order '$order', published '$(cat "$WITH_WINDOW/.launch/debug/plain/lucidos-app")'"
fi

echo "test: a failed window build fails the whole build and publishes nothing"
printf 'window old' > "$WITH_WINDOW/.launch/debug/plain/lucidos-app"
if out="$(run_build "$WITH_WINDOW" 1)"; then
    fail "the build reported success"
else
    pass "the build failed"
fi
if printf '%s\n' "$out" | grep -qx published \
    || [ "$(cat "$WITH_WINDOW/.launch/debug/plain/lucidos-app")" != "window old" ]; then
    fail "it published after a failed window build: $out"
else
    pass "the previous version stays whole"
fi

echo ""
echo "dev window launch binary: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
