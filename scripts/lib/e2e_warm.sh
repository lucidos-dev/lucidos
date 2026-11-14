#!/usr/bin/env bash
# e2e_warm.sh: run a GitHub e2e job's slow setup in the background, and wait
# for each piece only where a later step needs it. scripts/e2e-github-shard.sh
# starts the tasks; it and scripts/e2e-browser.sh wait for them.
#
# LUCIDOS_E2E_WARM_DIR names the task directory, and only the e2e workflow
# sets it. Each task writes <task>.started at once, <task>.log while it runs,
# and <task>.rc ("<exit status> <end epoch>") when it ends. Without the
# variable the wait is a no-op, so a local run never waits for a task.
#
# Tested by scripts/lib/e2e_warm_test.sh.

E2E_WARM_TIMEOUT_SECS="${E2E_WARM_TIMEOUT_SECS:-900}"
# What a failed wait exits with. A shard that dies on it tested nothing, so
# e2e_github_shard_rc reads it as no verdict. A test command's own code never
# takes this value.
E2E_WARM_FAILED_EXIT=77

e2e_warm_active() {
    [ -n "${LUCIDOS_E2E_WARM_DIR:-}" ]
}

# e2e_warm_start <task> <command...>: run the command in the background. A
# shell function works, since the background subshell inherits it. A runner
# keeps the task alive across steps until the job ends.
e2e_warm_start() {
    local task="$1" dir="${LUCIDOS_E2E_WARM_DIR:?e2e_warm_start needs LUCIDOS_E2E_WARM_DIR}"
    shift
    mkdir -p "$dir" || return 1
    rm -f "$dir/$task.rc"
    date +%s >"$dir/$task.started"
    (
        rc=0
        "$@" >"$dir/$task.log" 2>&1 </dev/null || rc=$?
        echo "$rc $(date +%s)" >"$dir/$task.rc.tmp" && mv "$dir/$task.rc.tmp" "$dir/$task.rc"
    ) >/dev/null 2>&1 &
}

# e2e_warm_wait [<task>...]: wait for the named tasks, or for every started
# one. Fails with E2E_WARM_FAILED_EXIT, and the end of the task's log, when a
# task failed, never started, or is still running after E2E_WARM_TIMEOUT_SECS.
e2e_warm_wait() {
    e2e_warm_active || return 0
    local dir="$LUCIDOS_E2E_WARM_DIR" task f began now rc ended started
    local -a tasks=("$@")
    if [ "$#" -eq 0 ]; then
        for f in "$dir"/*.started; do
            [ -e "$f" ] && tasks+=("$(basename "$f" .started)")
        done
    fi
    began="$(date +%s)"
    for task in ${tasks[@]+"${tasks[@]}"}; do
        if [ ! -f "$dir/$task.started" ]; then
            echo "ERROR: warm-up task $task was never started." >&2
            return "$E2E_WARM_FAILED_EXIT"
        fi
        while [ ! -f "$dir/$task.rc" ]; do
            now="$(date +%s)"
            if [ $((now - began)) -ge "$E2E_WARM_TIMEOUT_SECS" ]; then
                echo "ERROR: warm-up task $task is still running after ${E2E_WARM_TIMEOUT_SECS}s. Its log ends:" >&2
                tail -n 40 "$dir/$task.log" >&2 2>/dev/null
                return "$E2E_WARM_FAILED_EXIT"
            fi
            sleep 0.5
        done
        read -r rc ended <"$dir/$task.rc"
        if [ "$rc" != 0 ]; then
            echo "ERROR: warm-up task $task failed (exit $rc). Its log ends:" >&2
            tail -n 40 "$dir/$task.log" >&2
            return "$E2E_WARM_FAILED_EXIT"
        fi
        started="$(cat "$dir/$task.started")"
        echo "Warm-up: $task ready (ran $((ended - started))s, waited $(($(date +%s) - began))s)"
    done
}
