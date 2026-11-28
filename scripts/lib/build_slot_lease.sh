# shellcheck shell=bash

# Run a launch build in this shell while a broker in our process group holds
# the slot. The broker owns the lock, so daemons spawned by the build cannot
# inherit it. An older broker leaves the usual per-step wrappers in charge.
build_slot_leased() { # <label> <cmd...>
    local label="$1" owner scratch
    shift
    if [ -n "${LUCIDOS_BUILD_SLOT_HELD:-}" ]; then
        "$@"
        return
    fi
    # Bash 3.2 lacks BASHPID. A child reports the actual shell PID even
    # when the build runs in a subshell, where $$ names the outer shell.
    owner=${BASHPID:-$(exec sh -c 'printf "%s" "$PPID"')}
    if [ -z "$owner" ] || ! scratch="$(mktemp -d "${TMPDIR:-/tmp}/build-slot-lease.XXXXXX")"; then
        "$@"
        return
    fi
    if ! mkfifo "$scratch/ready" "$scratch/release" ||
        ! exec 3<>"$scratch/ready" 4<>"$scratch/release"; then
        rm -rf "$scratch"
        "$@"
        return
    fi
    local holder state="" slot="" jobs="" nice="" request_id="" class="" rc=0
    LUCIDOS_BUILD_SLOT_LEASE_OWNER="$owner" LUCIDOS_BUILD_SLOT_LEASE_RECORDS=1 \
        "$PROJECT_DIR/scripts/with-build-slot.sh" --label "$label" --lease \
        >&3 <&4 3>&- 4>&- &
    holder=$!
    # Open a read-only descriptor while FD 3 still keeps the FIFO open.
    # Close our writer before reading, so a dead helper gives us EOF.
    {
        exec 3>&-
        rm -rf "$scratch"
        IFS=' ' read -r state slot jobs nice request_id class || state=""
    } <"$scratch/ready"

    case "$state" in
        granted)
            local -x LUCIDOS_BUILD_SLOT_HELD="$slot"
            local -x LUCIDOS_BUILD_SLOT_LEASE_NICE="$nice"
            local -x LUCIDOS_BUILD_SLOT_REQUEST_ID="$request_id"
            local -x LUCIDOS_BUILD_SLOT_GRANTED_CLASS="$class"
            local -x LUCIDOS_BUILD_MEMORY_GATE_PASSED_AT
            LUCIDOS_BUILD_MEMORY_GATE_PASSED_AT="$(date +%s)"
            if [ -z "${CARGO_BUILD_JOBS:-}" ] && [ "$jobs" != "-" ]; then
                local -x CARGO_BUILD_JOBS="$jobs"
            fi
            local -x LUCIDOS_BUILD_SLOT_CLASS="" LUCIDOS_BUILD_SLOT_PRIORITY=""
            "$@" 4>&- || rc=$?
            printf 'release %s\n' "$rc" >&4
            ;;
        refused)
            rc="$slot"
            ;;
        *)
            "$@" 4>&- || rc=$?
            ;;
    esac
    exec 4>&-
    wait "$holder" 2>/dev/null || true
    return "$rc"
}
