#!/usr/bin/env bash
# measure-engine-build.sh: the benchmark behind the engine crate split's gate.
#
# Times what a coding agent pays for an engine change, so each split phase can
# prove its win (ADR 0392, docs/plans/2026-10-08-engine-crate-split-and-cold-builds.md).
#
# Usage:
#   scripts/measure-engine-build.sh [--cold] [--packages "<pkg> ..."]
#                                   [--out <dir>] [--compare <results.tsv> --match-jobs]
#                                   [--only "<step> ..."] [--fix-commit <rev>]
#                                   [--probe-set agents|core|stack] [--sample-host-rss]
#                                   [--per-crate] [--jobs-share N] [--record-lab]
#
#   --cold       Also time cold builds in a fresh temporary target dir.
#   --packages   The engine packages to build and test. Default: lucidos-engine.
#                A split phase lists every tier crate here.
#   --out        Where results.tsv and the cargo timing reports go.
#   --compare    A results.tsv from an earlier run. Compares wall time and peak RSS.
#   --match-jobs Use the recorded job count per step. Use on the same host.
#   --only       Space- or comma-separated step names. Use local_release_warm,
#                local_release_comment and local_release_landing for gateway builds.
#                local_release_bin_comment verifies binary source identity coverage.
#                local_release_<stack probe> times an Apply-profile edit.
#                Local-release edit probes warm the scratch checkout first.
#   --fix-commit An ancestor fix to revert and replay in the landing probe.
#   --probe-set agents  Measure the agent crate split's upper and lower modules.
#   --probe-set core    Measure the core split's mid-tier, core, and control modules.
#   --probe-set stack   Follow the Phase 8 probes and build the engine binary.
#   --per-crate         Write per-workspace-target wall time and RSS TSVs.
#   --jobs-share        Run with N Cargo jobs, within the build slot.
#   --sample-host-rss   Sample total rustc and clippy-driver RSS twice a second.
#   --record-lab        Run three passes and record one workspace result per step.
#
# Each probe inserts one private function into a file, then times an
# incremental build, test build and `make lint`'s clippy pair. Probe files must
# be clean at the start, and the script restores them on exit.
#
# Every cargo step takes its own build slot, and the steps run one at a time:
# two cargo runs in one worktree can exhaust the host's memory. Set
# CARGO_BUILD_JOBS to compare runs with the same number of jobs.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
source "$SCRIPT_DIR/lib/measure_probe.sh" || exit 1
cd "$PROJECT_DIR" || exit 1

# File tails, not paths, so a probe follows its module across a crate move.
PROBE_TAILS=(
    "src/engine/agent_session/mod.rs"
    "src/engine/chat/mod.rs"
    "src/engine/tools/mod.rs"
    "src/llm/mod.rs"
    "src/api/threads/mod.rs"
)

COLD=0
PACKAGES=""
OUT_DIR=""
COMPARE=""
MATCH_JOBS=0
ONLY=""
FIX_COMMIT=""
PROBE_SET=""
SAMPLE_HOST_RSS=0
PER_CRATE=0
JOBS_SHARE=""
RECORD_LAB=0
BENCHMARK_ARGS=("$@")
while [ $# -gt 0 ]; do
    case "$1" in
        --cold) COLD=1; shift ;;
        --record-lab) RECORD_LAB=1; shift ;;
        --packages) PACKAGES="${2:?--packages needs a value}"; shift 2 ;;
        --out) OUT_DIR="${2:?--out needs a value}"; shift 2 ;;
        --compare) COMPARE="${2:?--compare needs a value}"; shift 2 ;;
        --match-jobs) MATCH_JOBS=1; shift ;;
        --only) ONLY="${2:?--only needs step names}"; ONLY="${ONLY//,/ }"; shift 2 ;;
        --fix-commit) FIX_COMMIT="${2:?--fix-commit needs a value}"; shift 2 ;;
        --probe-set) PROBE_SET="${2:?--probe-set needs a value}"; shift 2 ;;
        --sample-host-rss) SAMPLE_HOST_RSS=1; shift ;;
        --per-crate) PER_CRATE=1; shift ;;
        --jobs-share) JOBS_SHARE="${2:?--jobs-share needs a value}"; shift 2 ;;
        -h | --help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
        *) echo "measure-engine-build.sh: unknown argument: $1" >&2; exit 2 ;;
    esac
done
case "$PROBE_SET" in
    "") ;;
    agents)
        PROBE_TAILS=(
            "src/engine/chat/mod.rs"
            "src/engine/agent_session/mod.rs"
            "src/api/threads/mod.rs"
            "src/engine/thread_events/mod.rs"
            "src/engine/event_wait/mod.rs"
            "src/llm/mod.rs"
        ) ;;
    core)
        PROBE_TAILS=(
            "src/engine/event_wait/mod.rs"
            "src/engine/command_guard.rs"
            "src/runtime/mod.rs"
            "src/engine/thread_events/mod.rs"
            "src/llm/mod.rs"
            "src/engine/chat/mod.rs"
        ) ;;
    stack)
        PROBE_TAILS=(
            "src/engine/thread_events/mod.rs"
            "src/engine/event_bus/mod.rs"
            "src/llm/openai/mod.rs"
            "src/runtime/mod.rs"
            "src/engine/command_guard.rs"
            "src/engine/event_wait/mod.rs"
            "src/engine/chat/mod.rs"
            "src/api/threads/mod.rs"
        ) ;;
    *) echo "unknown probe set: $PROBE_SET" >&2; exit 2 ;;
esac

if [ -n "$JOBS_SHARE" ]; then
    if [[ ! "$JOBS_SHARE" =~ ^[1-9][0-9]*$ ]] || [ "$MATCH_JOBS" -eq 1 ]; then
        echo "--jobs-share needs a positive integer and cannot be combined with --match-jobs" >&2
        exit 2
    fi
    export CARGO_BUILD_JOBS="$JOBS_SHARE"
fi
if [ "$PER_CRATE" -eq 1 ] && { [ ! -x /usr/bin/time ] || [ -n "${RUSTC_WORKSPACE_WRAPPER:-}" ]; }; then
    echo "--per-crate needs /usr/bin/time and an unset RUSTC_WORKSPACE_WRAPPER" >&2
    exit 2
fi

mkdir -p "$PROJECT_DIR/target" || exit 1
[ -n "$OUT_DIR" ] || OUT_DIR="$(mktemp -d "$PROJECT_DIR/target/measure-engine-build.XXXXXX")"
mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"
RESULTS="$OUT_DIR/results.tsv"
if [ -n "$COMPARE" ]; then
    COMPARE="$(cd "$(dirname "$COMPARE")" && pwd)/$(basename "$COMPARE")"
fi
if [ -n "$COMPARE" ] && { [ ! -s "$COMPARE" ] || [ "$COMPARE" = "$RESULTS" ]; }; then
    echo "--compare needs a separate, nonempty results file" >&2
    exit 2
fi
if [ "$MATCH_JOBS" -eq 1 ] && [ -z "$COMPARE" ]; then
    echo "--match-jobs requires --compare" >&2
    exit 2
fi
if [ -z "$PACKAGES" ]; then
    if [ "$PROBE_SET" = stack ]; then
        # The tier map is introduced with the stack guardrails.
        if [ -f crates/crate-tiers.tsv ]; then
            PACKAGES="$(awk '$3 == "engine" || $3 == "leaf" {print $1}' crates/crate-tiers.tsv)" || exit 1
        else
            PACKAGES="lucidos-core lucidos-engine lucidos-agents-api lucidos-engine-bin"
        fi
    else
        PACKAGES="lucidos-engine"
    fi
fi
PKG_ARGS=()
for pkg in $PACKAGES; do PKG_ARGS+=(-p "$pkg"); done
if [ "$RECORD_LAB" -eq 1 ]; then
    # shellcheck source=scripts/lib/lab_slot.sh
    source "$SCRIPT_DIR/lib/lab_slot.sh" || exit 1
    if [ -z "${LAB_BUILD_PASS:-}" ]; then
        LAB_KIND=build
        LAB_TOOLCHAIN="$(rustc --version)" || exit 1
        LAB_RUN="$(node -p 'require("node:crypto").randomUUID()')" || exit 1
        export LAB_KIND LAB_TOOLCHAIN LAB_RUN
        export LAB_OUT="$OUT_DIR"
        passes=()
        sample_count="$(node "$SCRIPT_DIR/scoreboard/lab-cli.mjs" sample-count)" || exit 1
        for ((sample = 1; sample <= sample_count; sample++)); do
            pass="$OUT_DIR/sample-$sample"
            LAB_BUILD_PASS="$sample" "$SCRIPT_DIR/measure-engine-build.sh" "${BENCHMARK_ARGS[@]}" --out "$pass" || exit $?
            passes+=("$pass/results.tsv")
        done
        node "$SCRIPT_DIR/scoreboard/lab-cli.mjs" record "$LAB_KIND" "$LAB_TOOLCHAIN" "$LAB_RUN" "$PACKAGES" "${passes[@]}"
        exit $?
    fi
fi

: > "$RESULTS" || exit 1
BUILD_ARGS=("${PKG_ARGS[@]}")
TEST_ARGS=(--lib)
if [ "$PROBE_SET" = stack ]; then
    BUILD_ARGS=(-p lucidos-engine-bin)
    TEST_ARGS=()
fi

probe_file() {
    local tail="$1"
    local paths
    paths="$(git ls-files "crates/lucidos-*/$tail")" || return
    if [ "$tail" = "src/runtime/mod.rs" ]; then
        # The API crate also carries a test-only runtime shim.
        printf '%s\n' "$paths" | while IFS= read -r candidate; do
            if [ -f "$candidate" ] && ! grep -q '^#!\[cfg(test)\]' "$candidate"; then
                printf '%s\n' "$candidate"
            fi
        done
    else
        printf '%s\n' "$paths"
    fi
}

PROBE_FILES=()
for tail in "${PROBE_TAILS[@]}"; do
    match="$(probe_file "$tail")" || exit 1
    if [ -z "$match" ] || [[ "$match" == *$'\n'* ]]; then
        echo "measure-engine-build.sh: expected exactly one tracked file for $tail" >&2
        exit 2
    fi
    PROBE_FILES+=("$match")
done

if ! git diff --quiet HEAD -- "${PROBE_FILES[@]}"; then
    echo "measure-engine-build.sh: a probe file has uncommitted changes:" >&2
    git diff --name-only HEAD -- "${PROBE_FILES[@]}" >&2
    exit 2
fi

COLD_TARGET=""
SCRATCH=""
PROBE_SEQ=0
selected() {
    case " $ONLY " in
        "  " | *" $1 "*) return 0 ;;
        *) return 1 ;;
    esac
}

# A cold target holds gigabytes, so a failed step must not leave it behind.
clean_up() {
    if [ "$PROBE_SEQ" -ne 0 ]; then git restore -- "${PROBE_FILES[@]}"; fi
    [ -z "$SCRATCH" ] || git worktree remove --force "$SCRATCH"
    [ -z "$COLD_TARGET" ] || rm -rf "$COLD_TARGET"
}
trap clean_up EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

probe() { # <file>
    PROBE_SEQ=$((PROBE_SEQ + 1))
    case "$1" in
        */src/runtime/mod.rs) measure_probe "$1" "$PROBE_SEQ" tests ;;
        *) measure_probe "$1" "$PROBE_SEQ" ;;
    esac
}

# Run inside the slot so queue time is excluded from the samples.
sample_host_rss() {
    while true; do
        ps -axo rss=,comm= | awk '$2 ~ /(^|\/)(rustc|clippy-driver)$/ { sum += $1 }
            END { print sum + 0 }'
        sleep 0.5
    done
}
export -f sample_host_rss

# Time one cargo command inside a build slot, excluding the wait for the slot.
step() { # <name> <command...>
    local name="$1"; shift
    local log="$OUT_DIR/$name.log"
    local report="${CARGO_TARGET_DIR:-target}/cargo-timings/cargo-timing.html"
    local time_flag=-v jobs
    if [ "$MATCH_JOBS" -eq 1 ]; then
        jobs="$(awk -F '\t' -v name="$name" '$1 == name && $3 ~ /^[1-9][0-9]*$/ { print $3; exit }' "$COMPARE")"
        if [ -n "$jobs" ]; then
            local CARGO_BUILD_JOBS="$jobs"
            export CARGO_BUILD_JOBS
        fi
    fi
    [ "$(uname -s)" != Darwin ] || time_flag=-l
    local wrapper_out=""
    if [ "$PER_CRATE" -eq 1 ]; then
        local MEASURE_RUSTC_DIR
        MEASURE_RUSTC_DIR="$(mktemp -d "$OUT_DIR/.rustc.XXXXXX")" || exit 1
        local MEASURE_RUSTC_CACHE="${RUSTC_WRAPPER-${CARGO_BUILD_RUSTC_WRAPPER-sccache}}"
        local RUSTC_WORKSPACE_WRAPPER="$SCRIPT_DIR/lib/measure_rustc.sh"
        local RUSTC_WRAPPER="$RUSTC_WORKSPACE_WRAPPER"
        export MEASURE_RUSTC_DIR MEASURE_RUSTC_CACHE RUSTC_WORKSPACE_WRAPPER RUSTC_WRAPPER
        wrapper_out="$OUT_DIR/$name.crates.tsv"
    fi
    rm -f "$report"
    # Peak RSS needs /usr/bin/time, which some Linux hosts lack (Debian's `time`
    # package). Without it the step still records wall time, and RSS reads `?`.
    local rss_timer=(env LC_ALL=C /usr/bin/time "$time_flag")
    [ -x /usr/bin/time ] || rss_timer=(env LC_ALL=C)
    local runner=("$SCRIPT_DIR/with-build-slot.sh" --label "measure-engine-build: $name" --)
    if [ "$RECORD_LAB" -eq 1 ]; then
        runner=(lab_step "build lab: $name")
    fi
    # shellcheck disable=SC2016
    "${runner[@]}" \
        bash -c '
            if [ "$1" -eq 1 ]; then
                sample_host_rss > "$2" &
                trap "kill $! 2>/dev/null; wait $! 2>/dev/null || true" EXIT
            fi
            shift 2
            uptime
            TIMEFORMAT="MEASURE_SECONDS %R"
            time "$@"
        ' _ "$SAMPLE_HOST_RSS" "$OUT_DIR/$name.rss" "${rss_timer[@]}" "$@" > "$log" 2>&1
    local rc=$?
    if [ -n "$wrapper_out" ]; then
        {
            printf 'package\ttarget_name\ttarget_type\twall_seconds\tpeak_rss_kib\n'
            find "$MEASURE_RUSTC_DIR" -name '*.tsv' -exec cat {} + | LC_ALL=C sort
        } > "$wrapper_out"
        rm -rf "$MEASURE_RUSTC_DIR"
    fi
    local secs
    secs="$(grep '^MEASURE_SECONDS ' "$log" | tail -1 | awk '{print $2}')"
    if [ "$rc" -ne 0 ]; then
        if [ "$RECORD_LAB" -eq 1 ] && [ "$rc" -eq 75 ]; then
            echo "Build lab deferred before $name. See $log" >&2
        else
            echo "measure-engine-build.sh: step $name failed (exit $rc), see $log" >&2
        fi
        exit "$rc"
    fi
    [ -f "$report" ] && cp "$report" "$OUT_DIR/$name.timing.html"
    # Prefer the caller's fixed count when the slot leaves it in place.
    local cores
    cores="${CARGO_BUILD_JOBS:-$(grep -o -E '[0-9]+ cores$' "$log" | head -1 | awk '{print $1}')}"
    local rss
    rss="$(awk '
        /maximum resident set size/ { printf "%.0f", $1 / 1024 }
        /Maximum resident set size/ { print $NF }
    ' "$log")"
    local host_rss="?"
    if [ "$SAMPLE_HOST_RSS" -eq 1 ]; then
        host_rss="$(awk '$1 > peak { peak = $1 } END { print peak + 0 }' "$OUT_DIR/$name.rss")"
    fi
    if [[ "$name" != prepare_* ]]; then
        printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$secs" "${cores:-?}" "${rss:-?}" "$host_rss" >> "$RESULTS"
    fi
    printf '%-34s %8ss  %3s cores  %s KiB peak RSS  %s KiB host RSS\n' \
        "$name" "$secs" "${cores:-?}" "${rss:-?}" "$host_rss"
}

run_build() { step "$1" cargo build --locked "${BUILD_ARGS[@]}" --timings; }
run_test() { step "$1" cargo test --locked "${PKG_ARGS[@]}" "${TEST_ARGS[@]+"${TEST_ARGS[@]}"}" --no-run --timings; }
run_clippy() { step "$1" make --no-print-directory lint-rust-clippy; }

prepare_warm() {
    local mode="$1" name="warm_$1"
    if [ "$RECORD_LAB" -eq 1 ]; then name="prepare_$name"; fi
    run_"$mode" "$name"
}

echo "packages: $PACKAGES"
echo "results:  $RESULTS"

if [ "$COLD" -eq 1 ]; then
    COLD_TARGET="$(mktemp -d "$PROJECT_DIR/target/measure-engine-build-target.XXXXXX")"
    if selected cold_build; then CARGO_TARGET_DIR="$COLD_TARGET" run_build cold_build; fi
    if selected cold_clippy; then CARGO_TARGET_DIR="$COLD_TARGET" run_clippy cold_clippy; fi
    rm -rf "$COLD_TARGET"
    COLD_TARGET="$(mktemp -d "$PROJECT_DIR/target/measure-engine-build-target.XXXXXX")"
    if selected cold_test; then CARGO_TARGET_DIR="$COLD_TARGET" run_test cold_test; fi
    rm -rf "$COLD_TARGET"
    COLD_TARGET=""
fi

probe_name() {
    case "$1" in
        */mod.rs) basename "$(dirname "$1")" ;;
        *) basename "$1" .rs ;;
    esac
}

run_stack_probes() {
    local mode="$1" file name
    local warmed=0
    if selected "warm_$mode"; then
        run_"$mode" "warm_$mode"
        warmed=1
    fi
    if [ "$mode" = build ] && selected noop_build; then
        [ "$warmed" -eq 1 ] || prepare_warm build
        run_build noop_build
        warmed=1
    fi
    for file in "${PROBE_FILES[@]}"; do
        name="edit_$(probe_name "$file")_$mode"
        if selected "$name"; then
            if [ "$warmed" -eq 0 ]; then
                prepare_warm "$mode"
                warmed=1
            fi
            probe "$file" || exit 1
            run_"$mode" "$name"
        fi
    done
    if [ "$PROBE_SEQ" -ne 0 ]; then
        git restore -- "${PROBE_FILES[@]}" || exit 1
        PROBE_SEQ=0
    fi
}

if [ "$PROBE_SET" = stack ]; then
    # Keep consecutive builds in the same Cargo mode. Interleaving modes
    # can carry a second file edit into the timed build.
    for mode in build test clippy; do run_stack_probes "$mode"; done
else
    if selected warm_build; then run_build warm_build; fi
    if selected warm_test; then run_test warm_test; fi
    if selected warm_clippy; then run_clippy warm_clippy; fi
    if selected noop_build; then
        if [ "$RECORD_LAB" -eq 1 ] && ! selected warm_build; then prepare_warm build; fi
        run_build noop_build
    fi
    for file in "${PROBE_FILES[@]}"; do
        module="$(probe_name "$file")"
        for mode in build test clippy; do
            if selected "edit_${module}_$mode"; then
                probe "$file" || exit 1
                run_"$mode" "edit_${module}_$mode"
            fi
        done
    done
fi

RELEASE_PROBES=()
if [ "$PROBE_SET" = stack ] && [ -n "$ONLY" ]; then
    for file in "${PROBE_FILES[@]}"; do
        module="$(probe_name "$file")"
        if selected "local_release_$module"; then
            RELEASE_PROBES+=("$file")
        fi
    done
fi

# Replay on one checkout and target so paths and the incremental cache stay
# fixed across each edit. The gateway builds all three packages together.
if { [ -n "$ONLY" ] && {
    selected local_release_warm || selected local_release_comment ||
        selected local_release_landing || selected local_release_bin_comment ||
        selected local_release_chat || [ "${#RELEASE_PROBES[@]}" -gt 0 ]
}; } || [ -n "$FIX_COMMIT" ]; then
    if selected local_release_landing; then
        if [ -z "$FIX_COMMIT" ] || ! git merge-base --is-ancestor "$FIX_COMMIT" HEAD; then
            echo "local_release_landing needs --fix-commit naming an ancestor of HEAD" >&2
            exit 2
        fi
        FIX_COMMIT="$(git rev-parse "$FIX_COMMIT^{commit}")"
    fi
    TARGET="${CARGO_TARGET_DIR:-$PROJECT_DIR/target}"
    case "$TARGET" in /*) ;; *) TARGET="$PROJECT_DIR/$TARGET" ;; esac
    mkdir -p "$PROJECT_DIR/target"
    SCRATCH="$(mktemp -d "$PROJECT_DIR/target/measure-engine-source.XXXXXX")"
    git worktree add --detach --quiet "$SCRATCH" HEAD || exit 1
    (
        cd "$SCRATCH" || exit 1
        export CARGO_TARGET_DIR="$TARGET"
        local_release() {
            local engine_pkg=lucidos-engine
            [ ! -f crates/lucidos-engine-bin/Cargo.toml ] || engine_pkg=lucidos-engine-bin
            step "$1" cargo build --locked -p "$engine_pkg" -p lucidos-gateway -p lucidos-cli \
                --profile local-release --timings
        }
        # The executable must report the checkout commit and a dirty suffix
        # precisely when one of the probes changes its source.
        check_build_id() {
            local step_name="$1" stamp="$2" state="$3" id
            id="$("$TARGET/local-release/lucidos-engine" --build-id)" || return 1
            case "$state:$id" in
                "clean:$stamp" | "dirty:$stamp-"?*)
                    printf '%s\n' "$id" > "$OUT_DIR/$step_name.build-id" ;;
                *) echo "$step_name: build id $id does not match $stamp ($state)" >&2; return 1 ;;
            esac
        }

        # Synthetic commits belong only to the scratch checkout.
        scratch_git() {
            git -c core.hooksPath=/dev/null -c user.name='Engine build benchmark' \
                -c user.email='benchmark@example.invalid' "$@"
        }

        if selected local_release_landing; then
            scratch_git revert --no-edit "$FIX_COMMIT" || exit 1
        fi
        local_release local_release_warm
        commit="$(git rev-parse --short HEAD)" || exit 1
        check_build_id local_release_warm "$commit" clean || exit 1
        if selected local_release_landing; then
            scratch_git cherry-pick "$FIX_COMMIT" || exit 1
            local_release local_release_landing
            commit="$(git rev-parse --short HEAD)" || exit 1
            check_build_id local_release_landing "$commit" clean || exit 1
        fi
        if [ "$PROBE_SET" != stack ] && [ -n "$ONLY" ] && selected local_release_chat; then
            chat_file="$(git ls-files 'crates/lucidos-engine/src/engine/chat/mod.rs' \
                'crates/lucidos-agents-api/src/engine/chat/mod.rs')" || exit 1
            [ -f "$chat_file" ] || { echo "expected one tracked chat module" >&2; exit 2; }
            printf '\n// Benchmark: change the chat module.\n' >> "$chat_file"
            local_release local_release_chat
            check_build_id local_release_chat "$commit" dirty || exit 1
            git restore -- "$chat_file" || exit 1
        fi
        if selected local_release_comment; then
            test_file=crates/lucidos-engine/src/engine/change_ops_tests.rs
            printf '\n// Benchmark: change a test-only comment.\n' >> "$test_file"
            local_release local_release_comment
            check_build_id local_release_comment "$commit" dirty || exit 1
            git restore -- "$test_file" || exit 1
        fi
        if [ -n "$ONLY" ] && selected local_release_bin_comment; then
            bin_file=crates/lucidos-engine-bin/src/main.rs
            [ -f "$bin_file" ] || { echo "$bin_file is missing" >&2; exit 2; }
            printf '\n// Benchmark: change a binary source comment.\n' >> "$bin_file"
            local_release local_release_bin_comment
            check_build_id local_release_bin_comment "$commit" dirty || exit 1
            git restore -- "$bin_file" || exit 1
        fi
        if [ "${#RELEASE_PROBES[@]}" -gt 0 ]; then
            if selected local_release_comment || selected local_release_bin_comment; then
                local_release local_release_reset
                check_build_id local_release_reset "$commit" clean || exit 1
            fi
            for file in "${RELEASE_PROBES[@]}"; do
                module="$(probe_name "$file")"
                probe "$file" || exit 1
                local_release "local_release_$module"
                check_build_id "local_release_$module" "$commit" dirty || exit 1
            done
        fi
    ) || exit $?
fi

if [ ! -s "$RESULTS" ]; then
    echo "measure-engine-build.sh: no steps selected" >&2
    exit 2
fi

if [ -n "$COMPARE" ]; then
    echo
    echo "change against $COMPARE:"
    awk -F'\t' 'NR == FNR { before[$1] = $2; cores[$1] = $3; rss[$1] = $4; host[$1] = $5; next }
        ($1 in before) && before[$1] > 0 {
            note = (cores[$1] == $3) ? "" : sprintf("  (cores %s -> %s, not comparable)", cores[$1], $3)
            if (rss[$1] > 0 && rss[$1] != "?" && $4 > 0 && $4 != "?")
                note = note sprintf("  RSS %+6.1f%%", 100 * ($4 - rss[$1]) / rss[$1])
            if (host[$1] + 0 > 0 && $5 + 0 > 0)
                note = note sprintf("  host RSS %+6.1f%%", 100 * ($5 - host[$1]) / host[$1])
            printf "%-34s %8.1fs -> %8.1fs  %+6.1f%%%s\n", $1, before[$1], $2, 100 * ($2 - before[$1]) / before[$1], note
        }' "$COMPARE" "$RESULTS"
fi
