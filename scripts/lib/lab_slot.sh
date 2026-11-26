# shellcheck shell=bash

lab_idle() {
    if [ -n "${LUCIDOS_BUILD_SLOT_HELD:-}" ]; then
        echo 'Run the lab outside a held build slot.' >&2
        return 2
    fi
    node "$SCRIPT_DIR/scoreboard/lab-cli.mjs" idle
}

lab_defer() {
    node "$SCRIPT_DIR/scoreboard/lab-cli.mjs" defer "$LAB_KIND" "$LAB_TOOLCHAIN" "$LAB_RUN" "$1"
}

lab_step() {
    local label="$1" request since rc=0
    shift
    lab_idle || {
        rc=$?
        if [ "$rc" -eq 75 ]; then lab_defer "$label" || return $?; fi
        return "$rc"
    }
    request="$(mktemp "$LAB_OUT/.slot.XXXXXX")" || return
    since="$(date -u +%Y-%m-%dT%H:%M:%SZ)" || return
    # shellcheck disable=SC2016
    LUCIDOS_BUILD_SLOT_CLASS=background "$SCRIPT_DIR/with-build-slot.sh" --label "$label" -- \
        bash -c '
            if [ "${LUCIDOS_BUILD_SLOT_GRANTED_CLASS:-}" != background ] ||
                [ -z "${LUCIDOS_BUILD_SLOT_REQUEST_ID:-}" ]; then
                echo "The lab needs a broker that records background slot grants." >&2
                exit 2
            fi
            printf "%s" "$LUCIDOS_BUILD_SLOT_REQUEST_ID" > "$1" || exit
            shift
            exec "$@"
        ' _ "$request" "$@" || rc=$?
    if [ "$rc" -eq 0 ]; then
        node "$SCRIPT_DIR/scoreboard/lab-cli.mjs" verify-release "$(cat "$request")" "$since" || rc=$?
    fi
    rm -f "$request"
    return "$rc"
}
