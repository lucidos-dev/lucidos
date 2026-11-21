#!/bin/bash
# Hermetic lease protocol tests. Only the stub broker can hold a lease.
set -eu

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/build_slot_lease.sh
source "$SCRIPT_DIR/build_slot_lease.sh"

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
export PROJECT_DIR="$scratch" TRACE="$scratch/trace"
unset CARGO_BUILD_JOBS LUCIDOS_BUILD_SLOT_HELD LUCIDOS_BUILD_SLOT_LEASE_NICE
unset LUCIDOS_BUILD_MEMORY_GATE_PASSED_AT LUCIDOS_BUILD_SLOT_CLASS LUCIDOS_BUILD_SLOT_PRIORITY
mkdir "$PROJECT_DIR/scripts"
cat > "$PROJECT_DIR/scripts/with-build-slot.sh" <<'BROKER'
#!/bin/bash
case "${MODE:-grant}" in
    unleased) echo unleased; exit 0 ;;
    error) echo error; exit 1 ;;
    dead) exit 1 ;;
    refused) echo 'refused 72'; exit 72 ;;
esac
[ "$LUCIDOS_BUILD_SLOT_LEASE_OWNER" = "$PPID" ] || exit 1
ps -o pgid= -p "$$" > "$PROJECT_DIR/broker.pgid"
echo "acquire ${LUCIDOS_BUILD_SLOT_CLASS:-none}" >> "$TRACE"
echo 'granted 2 4 10'
read -r _
echo released >> "$TRACE"
BROKER
chmod +x "$PROJECT_DIR/scripts/with-build-slot.sh"

assert_eq() {
    if [ "$1" != "$2" ]; then
        printf 'Expected [%s], got [%s]\n' "$1" "$2" >&2
        return 1
    fi
}

record_body() {
    # Use a subprocess to check what is actually exported.
    bash -c 'echo "body ${LUCIDOS_BUILD_SLOT_HELD:-none} ${CARGO_BUILD_JOBS:-none} ${LUCIDOS_BUILD_SLOT_LEASE_NICE:-none} ${LUCIDOS_BUILD_SLOT_CLASS:-none}"' >> "$TRACE"
    return "${BODY_RC:-0}"
}

reset_trace() { : > "$TRACE"; }

test_a_lease_exports_the_slot_and_the_share() {
    reset_trace
    LUCIDOS_BUILD_SLOT_CLASS=priority build_slot_leased engine record_body
    assert_eq $'acquire priority\nbody 2 4 10 none\nreleased' "$(cat "$TRACE")"
    assert_eq unset "${CARGO_BUILD_JOBS-unset}"
    assert_eq unset "${LUCIDOS_BUILD_SLOT_HELD-unset}"
    reset_trace
    CARGO_BUILD_JOBS=8 build_slot_leased engine record_body
    assert_eq $'acquire none\nbody 2 8 10 none\nreleased' "$(cat "$TRACE")"
}

test_an_inherited_slot_takes_no_lease() {
    reset_trace
    LUCIDOS_BUILD_SLOT_HELD=1 build_slot_leased engine record_body
    assert_eq 'body 1 none none none' "$(cat "$TRACE")"
}

test_a_failed_lease_still_builds() {
    local mode
    for mode in error dead; do
        reset_trace
        MODE="$mode" build_slot_leased engine record_body
        assert_eq 'body none none none none' "$(cat "$TRACE")"
    done
}

test_a_refused_gate_starts_no_build() {
    reset_trace
    local rc=0
    MODE=refused build_slot_leased engine record_body || rc=$?
    assert_eq 72 "$rc"
    assert_eq '' "$(cat "$TRACE")"
}

test_a_lease_releases_when_the_body_fails() {
    reset_trace
    local rc=0
    BODY_RC=6 build_slot_leased engine record_body || rc=$?
    assert_eq 6 "$rc"
    assert_eq $'acquire none\nbody 2 4 10 none\nreleased' "$(cat "$TRACE")"
}

test_the_build_shell_keeps_its_priority() {
    local before
    before="$(ps -o nice= -p "$$")"
    build_slot_leased engine record_body
    assert_eq "$before" "$(ps -o nice= -p "$$")"
}

test_the_lease_helper_shares_the_build_process_group() {
    build_slot_leased engine record_body
    assert_eq "$(ps -o pgid= -p "$$")" "$(cat "$PROJECT_DIR/broker.pgid")"
}

test_a_subshell_owns_its_lease() {
    reset_trace
    (build_slot_leased engine record_body)
    assert_eq $'acquire none\nbody 2 4 10 none\nreleased' "$(cat "$TRACE")"
}

test_a_lease_exports_the_slot_and_the_share
test_an_inherited_slot_takes_no_lease
test_a_failed_lease_still_builds
test_a_refused_gate_starts_no_build
test_a_lease_releases_when_the_body_fails
test_the_build_shell_keeps_its_priority
test_the_lease_helper_shares_the_build_process_group
test_a_subshell_owns_its_lease
echo 'build_slot_lease_test: passed'
