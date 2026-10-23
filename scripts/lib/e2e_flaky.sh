#!/usr/bin/env bash
# e2e_flaky.sh: the engine suite's one rerun, and the block that reports it
# (docs/plans/2026-10-16-main-check-fewer-reds-cheaper-fixes.md).
#
# Playwright already retries a failed browser test once. The engine suite has
# no retry, so a shard reruns the tests that failed, once, by exact name. A
# test that passes the rerun is flaky: the job passes, and this block names it
# in the shard's log. scripts/lib/main_check.sh reads the block.
#
# Sourced by scripts/e2e-github-shard.sh, scripts/lib/main_check.sh and
# scripts/lib/harden_suites.sh, whose retry round names the same failures.
# Tested by scripts/lib/e2e_flaky_test.sh.

# The block a rerun that passed leaves in the log: one test name per line.
E2E_FLAKY_ENGINE_MARK="── flaky: engine ──"
E2E_FLAKY_END_MARK="── end flaky ──"

# e2e_failed_engine_tests <log>: the tests a `cargo test` run failed, one name
# per line. Nothing when the run died another way (a compile error, a crash, a
# timeout), or when the failure list does not add up to the summary's count:
# those failures are never rerun.
e2e_failed_engine_tests() {
    local log="$1" count names
    count="$(sed -n 's/^test result: FAILED\..* \([0-9][0-9]*\) failed.*/\1/p' "$log" | tail -1)"
    [ -n "$count" ] || return 0
    names="$(awk '
        /^failures:$/ { listing = 1; names = ""; next }
        listing && /^    [^ ]/ { sub(/^    /, ""); names = names $0 "\n"; next }
        listing && NF == 0 { if (names != "") listing = 0; next }
        listing { listing = 0 }
        END { printf "%s", names }
    ' "$log")"
    [ "$(printf '%s' "$names" | grep -c .)" = "$count" ] || return 0
    printf '%s' "$names"
}

# e2e_rerun_failed_engine_tests <log> <test-engine.sh>: rerun what <log>'s run
# failed, once. Appends the rerun to <log>. Succeeds only when every named test
# ran and passed, and then ends the log with the flaky block.
e2e_rerun_failed_engine_tests() {
    local log="$1" runner="$2" names rerun="$1.rerun" count name
    local -a args=()
    names="$(e2e_failed_engine_tests "$log")"
    [ -n "$names" ] || return 1
    while IFS= read -r name; do args+=("$name"); done <<EOF
$names
EOF
    count="${#args[@]}"
    echo ""
    echo "── rerunning $count failed engine test(s), once ──"
    "$runner" -- -- "${args[@]}" --exact >"$rerun" 2>&1
    local rc=$?
    tee -a "$log" <"$rerun"
    # A name that matched nothing exits 0 with no test run: a pass nobody saw.
    if [ "$rc" -ne 0 ] || ! grep -q "^test result: ok\. $count passed;" "$rerun"; then
        rm -f "$rerun"
        return 1
    fi
    rm -f "$rerun"
    {
        echo "$E2E_FLAKY_ENGINE_MARK"
        printf '%s\n' "${args[@]}"
        echo "$E2E_FLAKY_END_MARK"
    } | tee -a "$log"
}
