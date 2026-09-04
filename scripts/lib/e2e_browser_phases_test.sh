#!/bin/bash
# Tests for the mobile-webkit phase split in scripts/e2e-browser.sh.
# Run: ./scripts/lib/e2e_browser_phases_test.sh   (no harness; direct, like host_memory_guard_test.sh)
#
# e2e-browser.sh is a SCRIPT, not a library: sourcing it would set up an e2e
# session and launch Playwright. So the three functions under test are lifted out
# with sed and sourced alone, the way build_dmg_test.sh, install_test.sh and
# release_abandon_test.sh already lift functions out of their scripts. Every lift
# is checked, so a rename fails this suite loudly rather than silently testing
# nothing.
#
# What it pins is the ORDER. The cheap CC-subprocess phase runs first and the
# expensive navigation phase second, so a shortfall lands in nav, where a partial
# chunk range carries over. Reversed, the 10 CC specs went months without a WebKit
# verdict because the guard kept ending the run at the phase boundary.
#
# Hermetic: no Playwright, no browser, no host-memory read. Every collaborator is
# a stub that records what it was asked to do, into one ordered trace.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BROWSER_SH="$PROJECT_DIR/scripts/e2e-browser.sh"

SANDBOX="$(mktemp -d)"
cleanup() { rm -rf "$SANDBOX"; }
trap cleanup EXIT

OUT="$SANDBOX/out"
mkdir -p "$OUT"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }

assert_eq() {
    local expected="$1" actual="$2" msg="$3"
    if [ "$expected" = "$actual" ]; then
        pass "$msg"
    else
        fail "$msg (expected '$expected', got '$actual')"
    fi
}

assert_says() {
    local file="$1" needle="$2" msg="$3"
    if grep -qF "$needle" "$file"; then
        pass "$msg"
    else
        fail "$msg (no '$needle' in the trace)"
        sed 's/^/      | /' "$file"
    fi
}

assert_silent_about() {
    local file="$1" needle="$2" msg="$3"
    if grep -qF "$needle" "$file"; then
        fail "$msg (found '$needle')"
        sed 's/^/      | /' "$file"
    else
        pass "$msg"
    fi
}

# The order assertion. Both needles must be present AND in this sequence, so a
# missing one fails rather than reading as ordered.
assert_before() {
    local file="$1" first="$2" second="$3" msg="$4" a b
    a="$(grep -nF -m1 "$first" "$file" | cut -d: -f1)"
    b="$(grep -nF -m1 "$second" "$file" | cut -d: -f1)"
    if [ -n "$a" ] && [ -n "$b" ] && [ "$a" -lt "$b" ]; then
        pass "$msg"
    else
        fail "$msg ('$first' at ${a:-missing}, '$second' at ${b:-missing})"
        sed 's/^/      | /' "$file"
    fi
}

# ── lift the functions under test out of the script ─────────────────────
LIFTED="$SANDBOX/lifted.sh"
: > "$LIFTED"

lift() {
    local fn="$1" body depth
    body="$(sed -n "/^$fn() {/,/^}/p" "$BROWSER_SH")"
    if [ -z "$body" ]; then
        echo "FATAL: could not lift $fn() out of scripts/e2e-browser.sh." >&2
        echo "It was renamed or reshaped. Fix this test rather than deleting it:" >&2
        echo "a lift that silently returns nothing tests nothing." >&2
        exit 1
    fi
    # sed stops at the FIRST column-0 `}`, so a flush-left brace inside the
    # function (an unindented awk program, a heredoc terminator) truncates the
    # lift. Three nets catch that, and this is the cheapest: an unbalanced lift
    # is refused here, a syntactically broken one dies at `source`, and a
    # syntactically valid half-function fails the behavioural assertions below.
    # All three functions balance exactly today.
    depth="$(printf '%s\n' "$body" | awk '{ o = gsub(/\{/, "{"); c = gsub(/\}/, "}"); d += o - c } END { print d + 0 }')"
    if [ "$depth" != "0" ]; then
        echo "FATAL: the lift of $fn() is unbalanced (brace depth $depth)." >&2
        echo "A column-0 '}' inside the function truncated it, so the suite would" >&2
        echo "drive a partial function. Indent that brace or widen the lift." >&2
        exit 1
    fi
    printf '%s\n\n' "$body" >> "$LIFTED"
}

lift merge_rc
lift run_specs_chunked
lift _run_browser_project_body

# shellcheck source=/dev/null
source "$LIFTED"

# ── the globals the lifted code reads ───────────────────────────────────
# shellcheck disable=SC2034 # read by the lifted functions, not from this file
HOST_MEMORY_STOP_EXIT=71
MEMORY_STOPPED=""
# shellcheck disable=SC2034 # read by the lifted functions, not from this file
TEST_FILE=""
# shellcheck disable=SC2034 # read by the lifted functions, not from this file
PW_ARGS=()
# shellcheck disable=SC2034 # read by the lifted functions, not from this file
CMD=(npx playwright test)
OUTPUT_ARG=()

# ── the stubs ───────────────────────────────────────────────────────────
# Each one echoes, and the driver captures stdout, so the trace is ONE ordered
# file: phase headers, chunk headers, Playwright invocations and boundary checks
# interleaved exactly as they happened. Order assertions read line numbers off it.

STUB_PW_RC=0
STUB_BOUNDARY_FAIL_AT=""

reset_stubs() {
    STUB_PW_RC=0
    STUB_BOUNDARY_FAIL_AT=""
}

# shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
playwright_file_filter() { printf '/%s$' "$1"; }

# Always assigns exactly one element. An EMPTY OUTPUT_ARG would trip `set -u` the
# moment the lifted code expands "${OUTPUT_ARG[@]}", on macOS bash 3.2.
# shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
set_output_dir() { OUTPUT_ARG=(--output="stub-output/$1"); }

# shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
run_playwright() {
    echo "playwright: $*"
    return "$STUB_PW_RC"
}

# Fails at exactly one named boundary, so a test can place the stop where it
# wants it and leave every other boundary green.
# shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
check_host_memory_at_boundary() {
    echo "boundary: $1"
    [ "$1" = "$STUB_BOUNDARY_FAIL_AT" ] && return 1
    return 0
}

# ── the fixture ─────────────────────────────────────────────────────────
# Two CC specs, three nav specs, and one *-desktop.spec.ts that also calls the CC
# helper. The desktop one must be excluded before the partition, or a chunk
# landing entirely on testIgnore'd files exits "no tests found".
FAKE="$SANDBOX/app"
mkdir -p "$FAKE/e2e"
printf 'await pickComposeDestination(page)\n' > "$FAKE/e2e/coding-agent.spec.ts"
printf 'await pickComposeDestination(page)\n' > "$FAKE/e2e/model-switching.spec.ts"
printf 'await page.goto("/")\n' > "$FAKE/e2e/chat.spec.ts"
printf 'await page.goto("/")\n' > "$FAKE/e2e/drafts.spec.ts"
printf 'await page.goto("/")\n' > "$FAKE/e2e/scroll-memory.spec.ts"
printf 'await pickComposeDestination(page)\n' > "$FAKE/e2e/settings-backup-navigation-desktop.spec.ts"

NAV_ONLY="$SANDBOX/nav-only"
mkdir -p "$NAV_ONLY/e2e"
printf 'await page.goto("/")\n' > "$NAV_ONLY/e2e/chat.spec.ts"
printf 'await page.goto("/")\n' > "$NAV_ONLY/e2e/drafts.spec.ts"

# Chunk size 2 against the fixture: CC is one chunk, nav is two. That gives the
# nav phase an INTERNAL boundary, which is where a real shortfall lands.
export LUCIDOS_E2E_WEBKIT_CHUNK=2

# Run the body in a spec directory and capture the whole trace. The lifted code
# globs `e2e/*.spec.ts` relative to cwd, exactly as e2e-browser.sh does from
# crates/lucidos-app.
drive_in() {
    local dir="$1" out="$2" prev="$PWD" rc=0
    MEMORY_STOPPED=""
    # shellcheck disable=SC2034 # cleared per run; set_output_dir refills it for the lifted code
    OUTPUT_ARG=()
    cd "$dir" || return 99
    _run_browser_project_body mobile-webkit >"$out" 2>&1 || rc=$?
    cd "$prev" || return 99
    return "$rc"
}

# ── Test 1: the order, which is the whole point ─────────────────────────
test_the_cc_phase_runs_first_and_nav_second() {
    echo "test: phase 1 is the CC-subprocess half, phase 2 is navigation"
    local rc
    reset_stubs
    drive_in "$FAKE" "$OUT/order.out"
    rc=$?
    assert_eq "0" "$rc" "a clean run returns 0"

    assert_says "$OUT/order.out" "phase 1/2: 2 CC-subprocess specs (sharded)" "phase 1 is labelled CC, with its count"
    assert_says "$OUT/order.out" "phase 2/2: 3 navigation specs (sharded)" "phase 2 is labelled navigation, with its count"
    assert_before "$OUT/order.out" \
        "phase 1/2: 2 CC-subprocess specs" \
        "phase 2/2: 3 navigation specs" \
        "the CC label is printed before the navigation label"

    # The labels could be right while the calls are swapped, so pin the WORK too.
    assert_before "$OUT/order.out" \
        "mobile-webkit CC chunk 1/1" \
        "mobile-webkit nav chunk 1/2" \
        "the CC specs actually run before the navigation specs"
}

test_both_phases_still_shard() {
    echo "test: both phases go through the chunk loop, not one big pass"
    reset_stubs
    drive_in "$FAKE" "$OUT/shard.out" || true
    assert_says "$OUT/shard.out" "mobile-webkit CC chunk 1/1: 2 specs (fresh browser)" "the CC phase is sharded"
    assert_says "$OUT/shard.out" "mobile-webkit nav chunk 1/2: 2 specs (fresh browser)" "the navigation phase is sharded"
    assert_says "$OUT/shard.out" "mobile-webkit nav chunk 2/2: 1 specs (fresh browser)" "the navigation remainder is its own chunk"
}

test_desktop_specs_are_excluded_from_both_phases() {
    echo "test: a *-desktop.spec.ts never reaches a chunk"
    reset_stubs
    drive_in "$FAKE" "$OUT/desktop.out" || true
    # The fixture's desktop spec calls the CC helper, so an exclusion applied
    # after the partition instead of before it would put it in phase 1.
    assert_silent_about "$OUT/desktop.out" "settings-backup-navigation-desktop" \
        "the desktop spec is excluded before the partition, not after"
    assert_says "$OUT/desktop.out" "phase 1/2: 2 CC-subprocess specs" "the CC count excludes it"
}

# ── Test 2: the boundary between the phases still stops the run ─────────
test_a_stop_at_the_phase_boundary_skips_navigation() {
    echo "test: a stop at the CC/nav boundary skips phase 2 and returns"
    local rc
    reset_stubs
    STUB_BOUNDARY_FAIL_AT="mobile-webkit phase 1/2 (CC)"
    drive_in "$FAKE" "$OUT/stop.out"
    rc=$?
    assert_eq "71" "$rc" "the memory-stop code is returned"
    assert_says "$OUT/stop.out" "boundary: mobile-webkit phase 1/2 (CC)" "the boundary is checked between the phases"
    assert_says "$OUT/stop.out" "phase 2/2 SKIPPED: stopped on host memory" "the skip is announced"
    assert_silent_about "$OUT/stop.out" "mobile-webkit nav chunk" "no navigation spec ran after the stop"
    assert_eq "mobile-webkit" "$MEMORY_STOPPED" "the project is recorded as stopped"
    # The CC half still got its verdict, which is the entire reason it goes first.
    assert_says "$OUT/stop.out" "mobile-webkit CC chunk 1/1" "the CC phase still ran to completion"
}

# ── Test 3: a memory stop never masks a real failure ────────────────────
test_a_failing_cc_phase_outranks_the_boundary_stop() {
    echo "test: a red CC phase is not overwritten by a stop at the boundary"
    local rc
    reset_stubs
    STUB_PW_RC=1
    STUB_BOUNDARY_FAIL_AT="mobile-webkit phase 1/2 (CC)"
    drive_in "$FAKE" "$OUT/red.out"
    rc=$?
    assert_eq "1" "$rc" "the test failure wins over exit 71"
}

# ── Test 4: the shortfall lands in nav, and CC keeps its verdict ────────
test_a_stop_inside_navigation_keeps_the_cc_verdict() {
    echo "test: a stop between navigation chunks leaves the CC phase reported"
    local rc
    reset_stubs
    STUB_BOUNDARY_FAIL_AT="mobile-webkit nav chunk 1/2"
    drive_in "$FAKE" "$OUT/shortfall.out"
    rc=$?
    assert_eq "71" "$rc" "the run reports the memory stop"
    assert_says "$OUT/shortfall.out" "mobile-webkit CC chunk 1/1" "the CC phase ran"
    assert_says "$OUT/shortfall.out" "mobile-webkit nav chunk 1/2" "navigation started"
    assert_silent_about "$OUT/shortfall.out" "mobile-webkit nav chunk 2/2" "navigation stopped at the boundary"
}

# ── Test 5: an unsplittable set falls back to one pass ──────────────────
test_a_set_with_no_cc_specs_runs_in_one_pass() {
    echo "test: with no CC specs the project runs unsplit, as before"
    local rc
    reset_stubs
    drive_in "$NAV_ONLY" "$OUT/single.out"
    rc=$?
    assert_eq "0" "$rc" "the single pass returns 0"
    assert_silent_about "$OUT/single.out" "phase 1/2" "no phase split is announced"
    assert_eq "1" "$(grep -c '^playwright:' "$OUT/single.out")" "exactly one Playwright invocation"
}

# ── Test 6: the durable guard, over the REAL spec inventory ─────────────
# The fixture proves the mechanism. This proves it against the specs that ship,
# so the day the partition or the order is edited back, the failure is here.
test_the_real_inventory_puts_cc_first() {
    echo "test: the shipped mobile-webkit inventory runs CC first"
    local e2e_dir f base cc=0 nav=0 first_chunk
    e2e_dir="$PROJECT_DIR/crates/lucidos-app/e2e"
    if [ ! -d "$e2e_dir" ]; then
        fail "spec dir not found: $e2e_dir"
        return
    fi
    for f in "$e2e_dir"/*.spec.ts; do
        [ -e "$f" ] || continue
        base="$(basename "$f")"
        case "$base" in *-desktop.spec.ts) continue ;; esac
        if grep -q "pickComposeDestination" "$f" 2>/dev/null; then
            cc=$((cc + 1))
        else
            nav=$((nav + 1))
        fi
    done
    # A disarmed check must not read as clean: with either half empty the body
    # falls through to the single pass and there is no order to test.
    if [ "$cc" -eq 0 ] || [ "$nav" -eq 0 ]; then
        fail "the shipped inventory no longer splits ($cc CC, $nav nav)"
        return
    fi

    reset_stubs
    drive_in "$PROJECT_DIR/crates/lucidos-app" "$OUT/real.out" || true

    assert_says "$OUT/real.out" "phase 1/2: $cc CC-subprocess specs (sharded)" \
        "phase 1 carries the CC count ($cc)"
    assert_says "$OUT/real.out" "phase 2/2: $nav navigation specs (sharded)" \
        "phase 2 carries the navigation count ($nav)"

    # The first chunk the run executes decides everything. If the order is put
    # back, this line names nav.
    first_chunk="$(grep -o 'mobile-webkit [A-Za-z]* chunk 1/[0-9]*' "$OUT/real.out" | head -1)"
    case "$first_chunk" in
        "mobile-webkit CC chunk 1/"*) pass "the first chunk executed is a CC chunk ($first_chunk)" ;;
        *) fail "the first chunk executed was '$first_chunk', not a CC chunk" ;;
    esac

    assert_before "$OUT/real.out" \
        "boundary: mobile-webkit phase 1/2 (CC)" \
        "mobile-webkit nav chunk 1/" \
        "the phase boundary is checked before navigation starts"
}

test_the_cc_phase_runs_first_and_nav_second
test_both_phases_still_shard
test_desktop_specs_are_excluded_from_both_phases
test_a_stop_at_the_phase_boundary_skips_navigation
test_a_failing_cc_phase_outranks_the_boundary_stop
test_a_stop_inside_navigation_keeps_the_cc_verdict
test_a_set_with_no_cc_specs_runs_in_one_pass
test_the_real_inventory_puts_cc_first

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
