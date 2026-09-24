#!/bin/bash
# Tests for the mobile-webkit phase split in scripts/e2e-browser.sh.
# Run: ./scripts/lib/e2e_browser_phases_test.sh   (no harness; direct, like host_memory_guard_test.sh)
#
# e2e-browser.sh is a SCRIPT, not a library: sourcing it would set up an e2e
# session and launch Playwright. So the functions under test are lifted out with
# sed and sourced alone, the way build_dmg_test.sh, install_test.sh and
# release_abandon_test.sh already lift functions out of their scripts. Every lift
# is checked, so a rename fails this suite loudly rather than silently testing
# nothing.
#
# What it pins is the ORDER. The cheap CC-subprocess phase runs first and the
# expensive navigation phase second, so a shortfall lands in nav, where a partial
# chunk range carries over. Reversed, the 10 CC specs went months without a WebKit
# verdict because the guard kept ending the run at the phase boundary.
#
# It also pins the TWO NARROWINGS, which share one rule: a narrowed run may never
# read as a complete project. LUCIDOS_E2E_WEBKIT_CHUNKS picks a chunk range inside
# nav, and LUCIDOS_E2E_WEBKIT_PHASE picks which phases run at all. Both stay on
# the real chunked path, both announce every skip, both restate themselves at the
# end, and both widen back on a value nobody can parse.
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
lift webkit_chunk_range
lift report_webkit_chunk_range
lift webkit_phase_selection
lift report_webkit_phase_selection
lift run_specs_chunked
lift _run_browser_project_body
lift run_browser_project

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
WEBKIT_CHUNK_RANGE_APPLIED=""
WEBKIT_PHASE_APPLIED=""

# ── the stubs ───────────────────────────────────────────────────────────
# Each one echoes, and the driver captures stdout, so the trace is ONE ordered
# file: phase headers, chunk headers, Playwright invocations and boundary checks
# interleaved exactly as they happened. Order assertions read line numbers off it.

STUB_PW_RC=0
STUB_BOUNDARY_FAIL_AT=""
# The in-chunk stop. STUB_TRIP_ON_CALL is the Playwright invocation (1-based)
# during which the sampler trips, and that call returns STUB_TRIP_RC, the code a
# SIGINTed runner exits with. STUB_FAIL_ON_CALL makes one call fail on its own.
STUB_TRIP_ON_CALL=""
STUB_TRIP_RC=130
STUB_FAIL_ON_CALL=""
PW_CALLS=0
TRIPPED=""

reset_stubs() {
    STUB_PW_RC=0
    STUB_BOUNDARY_FAIL_AT=""
    STUB_TRIP_ON_CALL=""
    STUB_TRIP_RC=130
    STUB_FAIL_ON_CALL=""
    PW_CALLS=0
    TRIPPED=""
}

# shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
playwright_file_filter() { printf '/%s$' "$1"; }

# Always assigns exactly one element. An EMPTY OUTPUT_ARG would trip `set -u` the
# moment the lifted code expands "${OUTPUT_ARG[@]}", on macOS bash 3.2.
# shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
set_output_dir() { OUTPUT_ARG=(--output="stub-output/$1"); }

# shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
run_playwright() {
    PW_CALLS=$((PW_CALLS + 1))
    echo "playwright: $*"
    if [ "$PW_CALLS" = "$STUB_TRIP_ON_CALL" ]; then
        TRIPPED=1
        return "$STUB_TRIP_RC"
    fi
    [ "$PW_CALLS" = "$STUB_FAIL_ON_CALL" ] && return 1
    return "$STUB_PW_RC"
}

# shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
host_memory_stopped_mid_chunk() {
    [ -n "$TRIPPED" ] || return 1
    # shellcheck disable=SC2034 # the real one sets it for report_memory_stop
    MEMORY_STOP_DETAIL="stub trip"
    return 0
}

# The tally of a project whose last invocation printed no summary: it cannot add
# up, so the real reporter returns 1. STUB_TALLY_RC stands in for that.
STUB_TALLY_RC=0
PW_TALLY_LOG="$SANDBOX/tally-stub"
# shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
report_playwright_totals() { echo "tally: $1"; return "$STUB_TALLY_RC"; }

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

# A wider fixture for the chunk-range cases: two CC specs (one chunk) and eight
# nav specs (four chunks at size 2). Four nav chunks is the smallest count where a
# range can have chunks skipped on BOTH sides of it, which is the shape a
# carry-over discharge actually takes.
RANGE="$SANDBOX/range"
mkdir -p "$RANGE/e2e"
printf 'await pickComposeDestination(page)\n' > "$RANGE/e2e/coding-agent.spec.ts"
printf 'await pickComposeDestination(page)\n' > "$RANGE/e2e/model-switching.spec.ts"
for n in 1 2 3 4 5 6 7 8; do
    printf 'await page.goto("/")\n' > "$RANGE/e2e/nav$n.spec.ts"
done

# Chunk size 2 against the fixture: CC is one chunk, nav is two. That gives the
# nav phase an INTERNAL boundary, which is where a real shortfall lands.
export LUCIDOS_E2E_WEBKIT_CHUNK=2

# Run the body in a spec directory and capture the whole trace. The lifted code
# globs `e2e/*.spec.ts` relative to cwd, exactly as e2e-browser.sh does from
# crates/lucidos-app.
drive_in() {
    local dir="$1" out="$2" prev="$PWD" rc=0
    MEMORY_STOPPED=""
    WEBKIT_CHUNK_RANGE_APPLIED=""
    WEBKIT_PHASE_APPLIED=""
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

# ── Test 7: the nav chunk range ─────────────────────────────────────────────
# LUCIDOS_E2E_WEBKIT_CHUNKS exists for two jobs: proving a guard change without an
# unfiltered pass, and discharging the tail a memory stop lost. Both are worthless
# if a ranged run can read as a complete one, so every case here checks that the
# narrowing is SAID OUT LOUD as well as applied.

test_the_range_parser_resolves_and_clamps() {
    echo "test: the range parser handles a range, an open end, and every bad value"
    assert_eq "1 4" "$(webkit_chunk_range '' 4 2>/dev/null)" "no value means every chunk"
    assert_eq "2 3" "$(webkit_chunk_range '2-3' 4 2>/dev/null)" "a closed range passes through"
    assert_eq "3 4" "$(webkit_chunk_range '3-' 4 2>/dev/null)" "an open end runs to the last chunk"
    assert_eq "1 4" "$(webkit_chunk_range '1-9' 4 2>/dev/null)" "a last past the end is clamped"
    # A FIRST past the end is a different case and must not clamp. Clamping "9-"
    # to "4 4" would run one chunk for a range naming none, which is the silent
    # reduction this knob must never do.
    assert_eq "1 4" "$(webkit_chunk_range '9-' 4 2>/dev/null)" "a first past the end runs everything"
    assert_eq "1 4" "$(webkit_chunk_range '9-10' 4 2>/dev/null)" "a range entirely past the end runs everything"
    # Every unusable value widens back to the full range. Running everything is
    # the safe direction; running nothing would report green having tested none.
    assert_eq "1 4" "$(webkit_chunk_range 'banana' 4 2>/dev/null)" "a non-range value runs everything"
    assert_eq "1 4" "$(webkit_chunk_range '3-2' 4 2>/dev/null)" "a backwards range runs everything"
    assert_eq "1 4" "$(webkit_chunk_range '0-2' 4 2>/dev/null)" "a zero first runs everything"
    assert_eq "1 4" "$(webkit_chunk_range '-3' 4 2>/dev/null)" "a missing first runs everything"
    assert_eq "1 4" "$(webkit_chunk_range 'a-b' 4 2>/dev/null)" "non-numeric bounds run everything"
    # Normalised, because the caller decides "did this narrow anything?" by
    # STRING-comparing these against 1 and nchunks. Unnormalised, `01-4` runs
    # every chunk and still reports itself as partial coverage.
    assert_eq "1 4" "$(webkit_chunk_range '01-4' 4 2>/dev/null)" "a leading-zero first is normalised"
    assert_eq "2 4" "$(webkit_chunk_range '2-04' 4 2>/dev/null)" "a leading-zero last is normalised"
    # `08` and `09` are the ones that matter: bash arithmetic reads a bare
    # leading-zero literal as octal and errors on those two digits.
    assert_eq "1 8" "$(webkit_chunk_range '01-08' 9 2>/dev/null)" "08 is read as decimal 8, not as an octal error"
    assert_eq "9 9" "$(webkit_chunk_range '09-' 9 2>/dev/null)" "09 is read as decimal 9, not as an octal error"
    # And it says so, rather than widening in silence. A typo that runs the whole
    # suite is safe; a typo nobody is told about is how it goes unnoticed.
    if webkit_chunk_range 'banana' 4 2>&1 >/dev/null | grep -q 'not a chunk range'; then
        pass "an unusable value is named on stderr"
    else
        fail "an unusable value widened silently"
    fi
}

test_no_range_runs_every_nav_chunk() {
    echo "test: with no range set the nav phase runs every chunk"
    local rc
    reset_stubs
    unset LUCIDOS_E2E_WEBKIT_CHUNKS
    drive_in "$RANGE" "$OUT/norange.out"
    rc=$?
    assert_eq "0" "$rc" "the full run returns 0"
    assert_says "$OUT/norange.out" "mobile-webkit nav chunk 1/4" "chunk 1 ran"
    assert_says "$OUT/norange.out" "mobile-webkit nav chunk 4/4" "chunk 4 ran"
    assert_silent_about "$OUT/norange.out" "SKIPPED, outside chunk range" "nothing was skipped"
    assert_silent_about "$OUT/norange.out" "LIMITED to chunks" "no narrowing was announced"
    assert_eq "" "$WEBKIT_CHUNK_RANGE_APPLIED" "the run is not recorded as ranged"
}

test_a_range_runs_only_its_members_and_says_so() {
    echo "test: a range runs its own chunks, skips the rest out loud, and restates itself"
    local rc
    reset_stubs
    export LUCIDOS_E2E_WEBKIT_CHUNKS=2-3
    drive_in "$RANGE" "$OUT/range.out"
    rc=$?
    unset LUCIDOS_E2E_WEBKIT_CHUNKS
    assert_eq "0" "$rc" "a ranged run returns 0 when its chunks pass"
    assert_says "$OUT/range.out" "LIMITED to chunks 2-3 of 4" "the narrowing is announced up front"
    assert_says "$OUT/range.out" "nav chunk 1/4: SKIPPED, outside chunk range 2-3" "chunk 1 says it was skipped"
    assert_says "$OUT/range.out" "nav chunk 4/4: SKIPPED, outside chunk range 2-3" "chunk 4 says it was skipped"
    assert_says "$OUT/range.out" "nav chunk 2/4: 2 specs (fresh browser)" "chunk 2 ran"
    assert_says "$OUT/range.out" "nav chunk 3/4: 2 specs (fresh browser)" "chunk 3 ran"
    # The CC phase is four chunks in the real inventory and always runs whole, so
    # a range can never cost the ten specs the phase order exists to protect.
    assert_says "$OUT/range.out" "mobile-webkit CC chunk 1/1" "the CC phase still ran in full"
    assert_eq "2-3 of 4" "$WEBKIT_CHUNK_RANGE_APPLIED" "the run records itself as ranged"

    # One boundary inside the range, and none after its last chunk: the chunks
    # past the range are not work this run was going to do, so stopping "before"
    # them would report a complete run as one cut short.
    assert_says "$OUT/range.out" "boundary: mobile-webkit nav chunk 2/4" "the boundary inside the range is checked"
    assert_silent_about "$OUT/range.out" "boundary: mobile-webkit nav chunk 3/4" "no boundary after the range's last chunk"

    # And the end-of-run report refuses to let it read as a whole project.
    report_webkit_chunk_range >"$OUT/rangereport.out" 2>&1
    assert_says "$OUT/rangereport.out" "CHUNK RANGE ONLY: 2-3 of 4" "the final report restates the range"
    assert_says "$OUT/rangereport.out" "Coverage is incomplete" "the final report calls the coverage incomplete"
}

test_an_open_ended_range_runs_to_the_last_chunk() {
    echo "test: an open-ended range discharges the tail a memory stop lost"
    reset_stubs
    export LUCIDOS_E2E_WEBKIT_CHUNKS=3-
    drive_in "$RANGE" "$OUT/openrange.out" || true
    unset LUCIDOS_E2E_WEBKIT_CHUNKS
    assert_says "$OUT/openrange.out" "LIMITED to chunks 3-4 of 4" "the open end resolves to the last chunk"
    assert_says "$OUT/openrange.out" "nav chunk 1/4: SKIPPED" "chunk 1 was skipped"
    assert_says "$OUT/openrange.out" "nav chunk 3/4: 2 specs" "chunk 3 ran"
    assert_says "$OUT/openrange.out" "nav chunk 4/4: 2 specs" "chunk 4 ran"
}

test_a_garbage_range_runs_every_chunk() {
    echo "test: an unparseable range runs everything rather than nothing"
    reset_stubs
    export LUCIDOS_E2E_WEBKIT_CHUNKS=chunks-2-through-3
    drive_in "$RANGE" "$OUT/junkrange.out" || true
    unset LUCIDOS_E2E_WEBKIT_CHUNKS
    assert_says "$OUT/junkrange.out" "mobile-webkit nav chunk 1/4" "chunk 1 ran"
    assert_says "$OUT/junkrange.out" "mobile-webkit nav chunk 4/4" "chunk 4 ran"
    assert_silent_about "$OUT/junkrange.out" "SKIPPED, outside chunk range" "nothing was skipped"
    assert_eq "" "$WEBKIT_CHUNK_RANGE_APPLIED" "a widened range is not recorded as a narrowing"
}

# The sibling knob is one character away, so a typo lands `2-3` in the SIZE. Bash
# arithmetic reads that as -1, nchunks goes negative, and the loop counts down
# forever. It must fall back rather than hang.
test_a_non_numeric_chunk_size_falls_back_instead_of_hanging() {
    echo "test: a range accidentally set as the chunk SIZE falls back to 3"
    local rc
    reset_stubs
    export LUCIDOS_E2E_WEBKIT_CHUNK=2-3
    drive_in "$RANGE" "$OUT/badsize.out"
    rc=$?
    export LUCIDOS_E2E_WEBKIT_CHUNK=2
    assert_eq "0" "$rc" "the run completes rather than hanging"
    # Ten specs at the fallback size of 3: CC is one chunk of 2, nav is three
    # chunks (3, 3, 2). A negative size would never have printed chunk 1 at all.
    assert_says "$OUT/badsize.out" "mobile-webkit nav chunk 1/3: 3 specs" "the fallback size of 3 is in force"
    assert_says "$OUT/badsize.out" "mobile-webkit nav chunk 3/3: 2 specs" "the remainder chunk is right"
}

test_a_full_width_range_is_not_announced_as_a_narrowing() {
    echo "test: an explicit range covering everything is not reported as incomplete"
    reset_stubs
    export LUCIDOS_E2E_WEBKIT_CHUNKS=1-4
    drive_in "$RANGE" "$OUT/fullrange.out" || true
    unset LUCIDOS_E2E_WEBKIT_CHUNKS
    assert_silent_about "$OUT/fullrange.out" "LIMITED to chunks" "a full-width range announces nothing"
    assert_eq "" "$WEBKIT_CHUNK_RANGE_APPLIED" "a full-width range is not a ranged run"
    report_webkit_chunk_range >"$OUT/fullreport.out" 2>&1
    assert_silent_about "$OUT/fullreport.out" "CHUNK RANGE ONLY" "the final report says nothing"
}

# ── Test 8: the phase selector ──────────────────────────────────────────────
# LUCIDOS_E2E_WEBKIT_CHUNKS narrows nav and leaves the CC phase whole, and three
# measurements put the CC phase at 93 to 97 percent of the memory a discharge
# costs. So the cheapest possible discharge, two nav specs, used to pay for all
# ten CC specs. LUCIDOS_E2E_WEBKIT_PHASE drops that half. It carries the same
# guarantees the range does: the real chunked path, every skip announced, and a
# final report that refuses to let a narrowed run read as a complete one.

test_the_phase_parser_resolves_and_widens() {
    echo "test: the phase parser takes nav, cc and both, and widens everything else"
    assert_eq "both" "$(webkit_phase_selection '' 2>/dev/null)" "no value means both phases"
    assert_eq "both" "$(webkit_phase_selection both 2>/dev/null)" "an explicit both passes through"
    assert_eq "nav" "$(webkit_phase_selection nav 2>/dev/null)" "nav passes through"
    assert_eq "cc" "$(webkit_phase_selection cc 2>/dev/null)" "cc passes through"
    # Every unusable value widens back. Running everything is the safe direction;
    # running nothing would report green having tested none.
    assert_eq "both" "$(webkit_phase_selection banana 2>/dev/null)" "an unknown word runs both"
    assert_eq "both" "$(webkit_phase_selection NAV 2>/dev/null)" "the match is exact, so NAV is not nav"
    assert_eq "both" "$(webkit_phase_selection 'nav cc' 2>/dev/null)" "a pair of values is not a selection"
    assert_eq "both" "$(webkit_phase_selection 1 2>/dev/null)" "a number is not a phase"
    # And it says so, rather than widening in silence.
    if webkit_phase_selection banana 2>&1 >/dev/null | grep -q 'is not nav, cc or both'; then
        pass "an unusable phase value is named on stderr"
    else
        fail "an unusable phase value widened silently"
    fi
}

test_no_phase_selection_runs_both_phases() {
    echo "test: with no phase set both phases run and nothing is called incomplete"
    reset_stubs
    unset LUCIDOS_E2E_WEBKIT_PHASE
    drive_in "$RANGE" "$OUT/nophase.out" || true
    assert_says "$OUT/nophase.out" "mobile-webkit CC chunk 1/1" "the CC phase ran"
    assert_says "$OUT/nophase.out" "mobile-webkit nav chunk 1/4" "the nav phase ran"
    assert_silent_about "$OUT/nophase.out" "PHASE=" "no phase narrowing was announced"
    assert_eq "" "$WEBKIT_PHASE_APPLIED" "the run is not recorded as phase-narrowed"
    report_webkit_phase_selection >"$OUT/nophasereport.out" 2>&1
    assert_silent_about "$OUT/nophasereport.out" "ONE PHASE ONLY" "the final report says nothing"
}

test_the_nav_phase_alone_runs_only_nav_and_says_so() {
    echo "test: PHASE=nav skips the CC phase out loud and keeps nav on the chunked path"
    local rc
    reset_stubs
    export LUCIDOS_E2E_WEBKIT_PHASE=nav
    drive_in "$RANGE" "$OUT/navonly.out"
    rc=$?
    unset LUCIDOS_E2E_WEBKIT_PHASE
    assert_eq "0" "$rc" "a nav-only run returns 0 when nav passes"
    assert_says "$OUT/navonly.out" "phase 1/2 SKIPPED: 2 CC-subprocess specs, LUCIDOS_E2E_WEBKIT_PHASE=nav" \
        "the skipped CC phase says so, with the count it did not run"
    assert_silent_about "$OUT/navonly.out" "mobile-webkit CC chunk" "no CC spec ran"
    # The whole point: nav stays on the REAL chunked path, so the boundary check,
    # the reaper and the peak sampler are all still live.
    assert_says "$OUT/navonly.out" "mobile-webkit nav chunk 1/4: 2 specs (fresh browser)" "nav chunk 1 ran in its own fresh browser"
    assert_says "$OUT/navonly.out" "mobile-webkit nav chunk 4/4: 2 specs (fresh browser)" "nav chunk 4 ran"
    assert_says "$OUT/navonly.out" "boundary: mobile-webkit nav chunk 1/4" "the boundary between nav chunks is still checked"
    # With no CC phase there is no CC-to-nav boundary to stand at, and a stop
    # needs work left to stop.
    assert_silent_about "$OUT/navonly.out" "boundary: mobile-webkit phase 1/2 (CC)" "no phase boundary is checked when the CC phase did not run"
    assert_eq "nav" "$WEBKIT_PHASE_APPLIED" "the run records itself as phase-narrowed"

    report_webkit_phase_selection >"$OUT/navonlyreport.out" 2>&1
    assert_says "$OUT/navonlyreport.out" "ONE PHASE ONLY: LUCIDOS_E2E_WEBKIT_PHASE=nav" "the final report restates the selection"
    assert_says "$OUT/navonlyreport.out" "Coverage is incomplete" "the final report calls the coverage incomplete"
    assert_says "$OUT/navonlyreport.out" "the CC-subprocess phase has no verdict" "the final report names the phase that has no verdict"
}

test_the_cc_phase_alone_runs_only_cc_and_says_so() {
    echo "test: PHASE=cc skips the navigation phase out loud"
    local rc
    reset_stubs
    export LUCIDOS_E2E_WEBKIT_PHASE=cc
    drive_in "$RANGE" "$OUT/cconly.out"
    rc=$?
    unset LUCIDOS_E2E_WEBKIT_PHASE
    assert_eq "0" "$rc" "a cc-only run returns 0 when the CC phase passes"
    assert_says "$OUT/cconly.out" "mobile-webkit CC chunk 1/1: 2 specs (fresh browser)" "the CC phase ran on the chunked path"
    assert_says "$OUT/cconly.out" "phase 2/2 SKIPPED: 8 navigation specs, LUCIDOS_E2E_WEBKIT_PHASE=cc" \
        "the skipped nav phase says so, with the count it did not run"
    assert_silent_about "$OUT/cconly.out" "mobile-webkit nav chunk" "no navigation spec ran"
    assert_silent_about "$OUT/cconly.out" "boundary: mobile-webkit phase 1/2 (CC)" "no phase boundary is checked when nav will not run"
    assert_eq "cc" "$WEBKIT_PHASE_APPLIED" "the run records itself as phase-narrowed"

    report_webkit_phase_selection >"$OUT/cconlyreport.out" 2>&1
    assert_says "$OUT/cconlyreport.out" "ONE PHASE ONLY: LUCIDOS_E2E_WEBKIT_PHASE=cc" "the final report restates the selection"
    assert_says "$OUT/cconlyreport.out" "the navigation phase has no verdict" "the final report names the phase that has no verdict"
}

test_a_garbage_phase_value_runs_both_phases() {
    echo "test: an unparseable phase runs everything rather than nothing"
    reset_stubs
    export LUCIDOS_E2E_WEBKIT_PHASE=navigation
    drive_in "$RANGE" "$OUT/junkphase.out" || true
    unset LUCIDOS_E2E_WEBKIT_PHASE
    assert_says "$OUT/junkphase.out" "mobile-webkit CC chunk 1/1" "the CC phase ran"
    assert_says "$OUT/junkphase.out" "mobile-webkit nav chunk 4/4" "the nav phase ran to the end"
    assert_says "$OUT/junkphase.out" "is not nav, cc or both" "the widening is named on stderr"
    assert_silent_about "$OUT/junkphase.out" "SKIPPED" "nothing was skipped"
    assert_eq "" "$WEBKIT_PHASE_APPLIED" "a widened phase is not recorded as a narrowing"
}

# The two knobs compose, and that pairing is the actual discharge recipe: the
# phase selector drops the expensive half, the range narrows what is left.
test_a_nav_only_run_composes_with_a_chunk_range() {
    echo "test: PHASE=nav and a chunk range narrow together, and both are reported"
    local rc
    reset_stubs
    export LUCIDOS_E2E_WEBKIT_PHASE=nav
    export LUCIDOS_E2E_WEBKIT_CHUNKS=3-4
    drive_in "$RANGE" "$OUT/navrange.out"
    rc=$?
    unset LUCIDOS_E2E_WEBKIT_PHASE
    unset LUCIDOS_E2E_WEBKIT_CHUNKS
    assert_eq "0" "$rc" "the narrowed run returns 0 when its chunks pass"
    assert_says "$OUT/navrange.out" "phase 1/2 SKIPPED" "the CC phase is skipped"
    assert_says "$OUT/navrange.out" "LIMITED to chunks 3-4 of 4" "the range is announced inside nav"
    assert_says "$OUT/navrange.out" "nav chunk 1/4: SKIPPED, outside chunk range 3-4" "chunk 1 says it was skipped"
    assert_says "$OUT/navrange.out" "nav chunk 3/4: 2 specs (fresh browser)" "chunk 3 ran"
    assert_says "$OUT/navrange.out" "nav chunk 4/4: 2 specs (fresh browser)" "chunk 4 ran"
    assert_says "$OUT/navrange.out" "boundary: mobile-webkit nav chunk 3/4" "the boundary inside the range is still checked"
    assert_eq "nav" "$WEBKIT_PHASE_APPLIED" "the phase narrowing is recorded"
    assert_eq "3-4 of 4" "$WEBKIT_CHUNK_RANGE_APPLIED" "the range narrowing is recorded too"

    # Both reports fire. Either one alone would understate what has no verdict.
    report_webkit_chunk_range >"$OUT/navrangereport.out" 2>&1
    report_webkit_phase_selection >>"$OUT/navrangereport.out" 2>&1
    assert_says "$OUT/navrangereport.out" "CHUNK RANGE ONLY: 3-4 of 4" "the range is restated"
    assert_says "$OUT/navrangereport.out" "ONE PHASE ONLY: LUCIDOS_E2E_WEBKIT_PHASE=nav" "the phase is restated"
    assert_eq "2" "$(grep -c 'Coverage is incomplete' "$OUT/navrangereport.out")" "both reports call the coverage incomplete"
}

# A set that cannot be split has no phases to choose between, so the selection
# runs everything. That must be said: a selection that silently ran the whole
# project is the same lie as a silent skip.
test_a_phase_selection_on_an_unsplittable_set_says_it_ran_everything() {
    echo "test: a phase selection on a set with no CC specs names what it did"
    reset_stubs
    export LUCIDOS_E2E_WEBKIT_PHASE=nav
    drive_in "$NAV_ONLY" "$OUT/nosplitphase.out" || true
    unset LUCIDOS_E2E_WEBKIT_PHASE
    assert_says "$OUT/nosplitphase.out" "did not split (0 CC, 2 nav)" "it names the counts it found"
    assert_says "$OUT/nosplitphase.out" "ran everything" "it says the selection did not narrow anything"
    assert_eq "1" "$(grep -c '^playwright:' "$OUT/nosplitphase.out")" "the single pass still ran"
    assert_eq "" "$WEBKIT_PHASE_APPLIED" "a selection that narrowed nothing is not recorded as a narrowing"
}

# ── the in-chunk stop ───────────────────────────────────────────────────
# The sampler interrupts a chunk that holds the freeze signature. The run must
# then read as a memory stop: exit 71, MEMORY_STOPPED set, nothing more started.

test_a_trip_inside_a_nav_chunk_stops_the_run_as_a_memory_stop() {
    echo "test: a trip inside nav chunk 1/2 stops the run, and nav chunk 2/2 never starts"
    local rc=0
    reset_stubs
    STUB_TRIP_ON_CALL=2
    drive_in "$FAKE" "$OUT/tripnav.out" || rc=$?
    assert_eq "71" "$rc" "the interrupted chunk's own code gives way to the memory stop"
    assert_eq "mobile-webkit" "$MEMORY_STOPPED" "the stop is recorded against the project"
    assert_says "$OUT/tripnav.out" "nav chunk 1/2: STOPPED inside the chunk on host memory" "the trace says where it stopped"
    assert_silent_about "$OUT/tripnav.out" "nav chunk 2/2: 1 specs" "the next chunk never starts"
    assert_silent_about "$OUT/tripnav.out" "boundary: mobile-webkit nav chunk 1/2" "no boundary check runs after a trip"
}

test_a_trip_inside_the_cc_phase_skips_navigation() {
    echo "test: a trip inside the CC phase skips the whole navigation phase"
    local rc=0
    reset_stubs
    STUB_TRIP_ON_CALL=1
    drive_in "$FAKE" "$OUT/tripcc.out" || rc=$?
    assert_eq "71" "$rc" "the run exits with the memory stop code"
    assert_says "$OUT/tripcc.out" "phase 2/2 SKIPPED: stopped on host memory" "navigation is skipped and says why"
    assert_eq "1" "$(grep -c '^playwright:' "$OUT/tripcc.out")" "only the interrupted chunk ran"
    assert_silent_about "$OUT/tripcc.out" "boundary: mobile-webkit phase 1/2 (CC)" "the phase boundary is not checked again"
}

test_a_failing_chunk_outranks_a_later_trip() {
    echo "test: a real failure before the trip keeps its code"
    local rc=0
    reset_stubs
    STUB_FAIL_ON_CALL=1
    STUB_TRIP_ON_CALL=2
    drive_in "$FAKE" "$OUT/failtrip.out" || rc=$?
    assert_eq "1" "$rc" "the failing CC chunk is not hidden by the stop"
    assert_eq "mobile-webkit" "$MEMORY_STOPPED" "the stop is still recorded"
}

test_a_trip_inside_a_single_pass_project_stops_it() {
    echo "test: a trip during a one-pass project reads as a memory stop"
    local rc=0 prev="$PWD"
    reset_stubs
    STUB_TRIP_ON_CALL=1
    MEMORY_STOPPED=""
    # shellcheck disable=SC2034 # cleared per run; set_output_dir refills it for the lifted code
    OUTPUT_ARG=()
    cd "$FAKE" || return
    _run_browser_project_body chromium >"$OUT/tripone.out" 2>&1 || rc=$?
    cd "$prev" || return
    assert_eq "71" "$rc" "the project exits with the memory stop code"
    assert_eq "chromium" "$MEMORY_STOPPED" "the stop is recorded against that project"
    assert_says "$OUT/tripone.out" "chromium: STOPPED inside the run on host memory" "the trace says it stopped"
}

# run_playwright itself, lifted under another name because the stub above owns
# the real one. It must record the runner's pid, and keep its old contract: the
# output reaches the terminal and the tally, and the code is Playwright's own.
REAL_RUN_PW="$SANDBOX/real-run-playwright.sh"
sed -n '/^run_playwright() {/,/^}/p' "$BROWSER_SH" | sed '1s/^run_playwright()/real_run_playwright()/' > "$REAL_RUN_PW"
if ! grep -q '^real_run_playwright() {' "$REAL_RUN_PW"; then
    echo "FATAL: could not lift run_playwright() out of scripts/e2e-browser.sh." >&2
    exit 1
fi

test_an_interrupted_project_still_exits_as_a_memory_stop() {
    echo "test: a tally broken by the interrupt does not turn the stop into a failure"
    local rc=0 prev="$PWD"
    reset_stubs
    STUB_TRIP_ON_CALL=1
    STUB_TALLY_RC=1
    MEMORY_STOPPED=""
    # shellcheck disable=SC2034 # cleared per run; set_output_dir refills it for the lifted code
    OUTPUT_ARG=()
    cd "$FAKE" || return
    run_browser_project chromium >"$OUT/tallytrip.out" 2>&1 || rc=$?
    assert_eq "71" "$rc" "the project still exits with the memory stop code"
    assert_says "$OUT/tallytrip.out" "did not report" "the tally says why it did not add up"
    reset_stubs
    STUB_TALLY_RC=1
    MEMORY_STOPPED=""
    rc=0
    run_browser_project chromium >"$OUT/tallyplain.out" 2>&1 || rc=$?
    cd "$prev" || return
    assert_eq "1" "$rc" "without a stop, a tally that does not add up still fails the project"
    STUB_TALLY_RC=0
}

test_run_playwright_does_not_start_after_a_trip() {
    echo "test: run_playwright refuses to start once the sampler has tripped"
    local rc=0 ran="$SANDBOX/ran"
    rm -f "$ran"
    (
        # shellcheck source=/dev/null
        source "$REAL_RUN_PW"
        # shellcheck disable=SC2034 # read by the lifted run_playwright
        TRIPPED=1
        real_run_playwright sh -c "touch '$ran'"
    ) >"$OUT/notstarted.out" 2>&1 || rc=$?
    assert_eq "71" "$rc" "it returns the memory stop code"
    if [ -e "$ran" ]; then fail "the command ran after a trip"; else pass "the command never ran"; fi
    assert_says "$OUT/notstarted.out" "does not start" "it says why"
}

test_run_playwright_records_its_runner_and_keeps_its_contract() {
    echo "test: run_playwright records the runner pid and keeps output and exit code"
    local rc=0 log="$SANDBOX/runner-log" pid
    reset_stubs
    : > "$log"
    (
        PW_TALLY_LOG="$SANDBOX/tally"
        : > "$PW_TALLY_LOG"
        # shellcheck source=/dev/null
        source "$REAL_RUN_PW"
        # shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
        record_host_memory_runner() { echo "record $1" >> "$log"; }
        # shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
        clear_host_memory_runner() { echo "clear" >> "$log"; }
        # shellcheck disable=SC2329 # a seam: invoked by the lifted code, not from this file
        interrupt_host_memory_runner_if_tripped() { echo "late-check" >> "$log"; }
        # shellcheck disable=SC2016 # expanded by the inner sh, not here
        real_run_playwright sh -c 'echo "pid=$$"; echo out-line; echo err-line >&2; exit 3'
    ) >"$OUT/realpw.out" 2>&1 || rc=$?
    assert_eq "3" "$rc" "the code is the command's own"
    assert_says "$OUT/realpw.out" "out-line" "stdout reaches the terminal"
    assert_says "$OUT/realpw.out" "err-line" "stderr reaches the terminal"
    assert_says "$SANDBOX/tally" "out-line" "stdout reaches the tally"
    assert_says "$SANDBOX/tally" "err-line" "stderr reaches the tally"
    pid="$(sed -n 's/^pid=//p' "$OUT/realpw.out")"
    assert_says "$log" "record $pid" "the recorded pid is the runner's own"
    assert_before "$log" "record" "clear" "the record is cleared once the runner returns"
    assert_before "$log" "record" "late-check" "a trip that landed during the start is checked once recorded"
}

test_the_cc_phase_runs_first_and_nav_second
test_both_phases_still_shard
test_desktop_specs_are_excluded_from_both_phases
test_a_stop_at_the_phase_boundary_skips_navigation
test_a_failing_cc_phase_outranks_the_boundary_stop
test_a_stop_inside_navigation_keeps_the_cc_verdict
test_a_set_with_no_cc_specs_runs_in_one_pass
test_the_real_inventory_puts_cc_first
test_the_range_parser_resolves_and_clamps
test_no_range_runs_every_nav_chunk
test_a_range_runs_only_its_members_and_says_so
test_an_open_ended_range_runs_to_the_last_chunk
test_a_garbage_range_runs_every_chunk
test_a_non_numeric_chunk_size_falls_back_instead_of_hanging
test_a_full_width_range_is_not_announced_as_a_narrowing
test_the_phase_parser_resolves_and_widens
test_no_phase_selection_runs_both_phases
test_the_nav_phase_alone_runs_only_nav_and_says_so
test_the_cc_phase_alone_runs_only_cc_and_says_so
test_a_garbage_phase_value_runs_both_phases
test_a_nav_only_run_composes_with_a_chunk_range
test_a_phase_selection_on_an_unsplittable_set_says_it_ran_everything
test_a_trip_inside_a_nav_chunk_stops_the_run_as_a_memory_stop
test_a_trip_inside_the_cc_phase_skips_navigation
test_a_failing_chunk_outranks_a_later_trip
test_a_trip_inside_a_single_pass_project_stops_it
test_run_playwright_records_its_runner_and_keeps_its_contract
test_an_interrupted_project_still_exits_as_a_memory_stop
test_run_playwright_does_not_start_after_a_trip

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
