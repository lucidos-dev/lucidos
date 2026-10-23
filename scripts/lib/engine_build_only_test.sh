#!/bin/bash
# Tests for the `--engine-build` path through scripts/web-dev.sh: a background
# engine rebuild compiles a binary and nothing else.
#
# A SOURCE SCAN, not a run. Running the real path would compile the engine, and
# the property under test is an ORDERING of top-level statements, which is
# exactly what a scan can read. The alternative on offer was a comment, and a
# comment is what was already there: web-dev.sh has claimed since it was written
# that build-only needs no ports, while calling `allocate_ports` eight lines
# above the exit that was supposed to precede it.
#
# What that cost, and why the ordering is load-bearing rather than tidy:
# `allocate_ports` reclaims stale listeners, kills unprotected ones, refuses to
# walk off a pinned port, and rewrites the registry. So a workspace whose
# engine.pid had gone stale read its OWN live engine as a foreign squatter, and
# every background rebuild died there in under a second, before cargo, behind a
# "Retry build" toast that replayed the same failure on every tap.
# See docs/plans/2026-09-18-engine-build-only-needs-no-ports.md.
#
# The RED cases are the load-bearing half. Every check runs twice: once over the
# real script, and once over a scratch copy doctored back into the shape that
# shipped the bug. A scan that can only go green is indistinguishable from no
# scan at all, which is the failure mode the comment it replaces already had.
#
# Run: ./scripts/lib/engine_build_only_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEB_DEV="$SCRIPT_DIR/../web-dev.sh"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# ── the scan ────────────────────────────────────────────────────────────
# Takes the script to read, so the suite can point it at a doctored copy. Every
# pattern is anchored to the start of a line, so the comments that discuss the
# ordering cannot satisfy the scan they describe.

line_of() { # <script> <ere>
    grep -nE "$2" "$1" 2>/dev/null | head -1 | cut -d: -f1
}

# Echoes one problem per line, and nothing when the script is correct. Pure, so
# both the real script and each doctored copy go through identical code.
scan_engine_build_only() { # <script>
    local script="$1" build_only allocate resolve prereqs

    # shellcheck disable=SC2016 # a grep pattern: $ENGINE_BUILD_ONLY is the text being matched
    build_only="$(line_of "$script" '^if \[ -n "\$ENGINE_BUILD_ONLY" \]; then')"
    allocate="$(line_of "$script" '^allocate_ports ')"
    resolve="$(line_of "$script" '^resolve_workspace$')"
    prereqs="$(line_of "$script" '^check_prereqs')"

    if [ -z "$build_only" ]; then
        echo "the build-only branch is gone, so nothing here can be checked"
        return 0
    fi
    for pair in "allocate_ports:$allocate" "resolve_workspace:$resolve" \
        "check_prereqs:$prereqs"; do
        [ -n "${pair#*:}" ] || echo "a statement the scan needs is gone: ${pair%%:*}"
    done

    # The reported bug. Everything else here is a way of keeping it fixed.
    [ -n "$allocate" ] && [ "$build_only" -gt "$allocate" ] &&
        echo "allocate_ports runs before the build-only exit"

    # resolve_workspace sets WORKSPACE, PG_NAME and the pidfile/log paths, and
    # build_or_find_engine runs from the workspace it resolves. So build-only
    # sits BETWEEN the two, never above both.
    [ -n "$resolve" ] && [ "$resolve" -gt "$build_only" ] &&
        echo "resolve_workspace no longer precedes the build-only exit"

    # The tool checks (cargo, node, cmake) are exactly what a compile needs, so
    # build-only keeps them. Only the Docker DAEMON check is skipped, inside
    # check_prereqs; preflight_test.sh owns that half.
    [ -n "$prereqs" ] && [ "$prereqs" -gt "$build_only" ] &&
        echo "check_prereqs no longer precedes the build-only exit"

    # The whole point of the branch is that nothing after it runs. One that fell
    # through would reach setup_postgres and start_gateway.
    sed -n "${build_only},\$p" "$script" | sed -n '1,8p' | grep -qE '^ *exit 0$' ||
        echo "the build-only branch no longer exits within its own block"

    # A bare check_prereqs here would hard-exit on a stopped Docker daemon,
    # which a compile never reaches.
    grep -qE '^check_prereqs "\$\{ENGINE_BUILD_ONLY:\+build-only\}"$' "$script" ||
        echo "the build-only mode is no longer passed to check_prereqs"

    return 0
}

expect_clean() { # <scan-output> <what-was-being-checked>
    if [ -z "$1" ]; then pass "$2"; else fail "$2 (flagged: $1)"; fi
}

expect_flagged() { # <scan-output> <substring-it-must-name> <what>
    case "$1" in
        *"$2"*) pass "$3" ;;
        "") fail "$3 (the scan reported nothing)" ;;
        *) fail "$3 (flagged the wrong thing: $1)" ;;
    esac
}

# Copy the real script with one `sed` applied, so each red case differs from
# the green one by exactly the regression it stands for.
doctor() { # <name> <sed-expr...>
    local name="$1"
    shift
    local out="$SCRATCH/$name.sh"
    local args=()
    local e
    for e in "$@"; do args+=(-e "$e"); done
    sed "${args[@]}" "$WEB_DEV" > "$out"
    printf '%s' "$out"
}

# ── green: the real script ──────────────────────────────────────────────

echo "the real web-dev.sh"
expect_clean "$(scan_engine_build_only "$WEB_DEV")" \
    "--engine-build compiles without touching ports"

# ── red: the shapes that shipped, or could ship, the bug ────────────────

echo "the regressions it has to catch"

# The exact pre-fix shape: allocate_ports hoisted back above the exit.
# shellcheck disable=SC2016 # sed text, not shell: the $ are sed anchors and a literal
moved="$(doctor allocate-above \
    '/^allocate_ports "\$WORKSPACE"$/d' \
    '/^resolve_workspace$/a\
allocate_ports "$WORKSPACE"')"
expect_flagged "$(scan_engine_build_only "$moved")" \
    "allocate_ports runs before the build-only exit" \
    "an allocate_ports hoisted back above the exit is caught"

# The reverse slip: the exit outran resolve_workspace, so build_or_find_engine
# runs with no WORKSPACE. MOVED rather than deleted, because deleting it lands
# on the missing-statement arm below and leaves this ordering branch unproven.
# shellcheck disable=SC2016 # sed text, not shell: the $ are sed anchors and a literal
early="$(doctor resolve-below \
    '/^resolve_workspace$/d' \
    '/^allocate_ports "\$WORKSPACE"$/a\
resolve_workspace')"
expect_flagged "$(scan_engine_build_only "$early")" \
    "resolve_workspace no longer precedes the build-only exit" \
    "a build-only exit that outran resolve_workspace is caught"

# Same slip on the third ordering branch, which otherwise no red case reaches.
# shellcheck disable=SC2016 # sed text, not shell: the $ are sed anchors and a literal
late_prereqs="$(doctor prereqs-below \
    '/^check_prereqs /d' \
    '/^allocate_ports "\$WORKSPACE"$/a\
check_prereqs "${ENGINE_BUILD_ONLY:+build-only}"')"
expect_flagged "$(scan_engine_build_only "$late_prereqs")" \
    "check_prereqs no longer precedes the build-only exit" \
    "a check_prereqs that fell below the exit is caught"

# A statement the scan reads gone entirely. Distinct from the two above: the
# scan cannot compare an ordering it cannot locate, and saying so beats
# reporting the script clean.
gone="$(doctor resolve-gone '/^resolve_workspace$/d')"
expect_flagged "$(scan_engine_build_only "$gone")" \
    "a statement the scan needs is gone: resolve_workspace" \
    "a vanished statement is named rather than passed over"

# The branch stops exiting, so build-only falls through into the full launch.
# A RANGE address, not `0,/re/`: that GNU form is a parse error on BSD sed, and
# a bare `s///` would hit the --engine-only branch's `exit 0` as well.
# shellcheck disable=SC2016 # sed text, not shell: $ENGINE_BUILD_ONLY is the line being matched
fell="$(doctor no-exit \
    '/^if \[ -n "\$ENGINE_BUILD_ONLY" \]; then/,/^fi$/s/^    exit 0$/    true/')"
expect_flagged "$(scan_engine_build_only "$fell")" \
    "no longer exits within its own block" \
    "a build-only branch that stopped exiting is caught"

# The mode argument dropped, so a stopped Docker daemon fails the rebuild again.
# Matched by line start rather than by the argument's text: BSD sed reads the
# `\{` of `${ENGINE_BUILD_ONLY:+…}` as an interval and errors on it.
bare="$(doctor bare-prereqs 's/^check_prereqs .*$/check_prereqs/')"
expect_flagged "$(scan_engine_build_only "$bare")" \
    "no longer passed to check_prereqs" \
    "a bare check_prereqs is caught"

echo ""
echo "engine_build_only_test.sh: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
