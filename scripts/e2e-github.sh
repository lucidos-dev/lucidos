#!/usr/bin/env bash
# e2e-github.sh: run e2e suites on GitHub's runners (ADR 0382).
#
#   ./scripts/e2e-github.sh <all|api|browser|wasm|embedder> [--commit <sha>] [the suite's own flags]
#   ./scripts/e2e-github.sh main-check --commit <sha>
#
# The e2e scripts hand over here unless the run must stay local (ADR 0386),
# so call it as `./scripts/e2e.sh` and friends. Flags mean what they mean locally:
#   all      --no-webkit
#   api      -f <filter>, and anything else is passed to cargo test
#   browser  -f <spec>, --webkit, --no-webkit, -- <playwright args>
#            (a *.spec.ts among the Playwright args narrows both legs to it)
#
# --commit tests that commit instead of HEAD. main-check is the main check's
# selection (scripts/main-check.sh): the engine suite plus every e2e suite.
#
# Specs with a test tagged @real-claude-code run here, as the local leg, in
# parallel with the remote run. Under --commit they do not run: the local leg
# could test only this checkout's files. Everything else runs on GitHub, where
# the browser shards answer Claude Code with the fake. The report and exit
# code match a local run; 76 means the run produced no verdict.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

suite="${1:-}"
shift || true
case "$suite" in
    all | api | browser | wasm | embedder | main-check) ;;
    *)
        echo "usage: $0 <all|api|browser|wasm|embedder|main-check> [--commit <sha>] [flags]" >&2
        exit 1
        ;;
esac

# The release libs are maintainer-only, so the preflight below names what is
# missing rather than failing on the source line.
if [ -f "$SCRIPT_DIR/lib/release_tree.sh" ]; then
    # shellcheck source=scripts/lib/release_tree.sh
    source "$SCRIPT_DIR/lib/release_tree.sh"
fi
# shellcheck source=scripts/lib/e2e.sh
source "$SCRIPT_DIR/lib/e2e.sh"
# shellcheck source=scripts/lib/e2e_github.sh
source "$SCRIPT_DIR/lib/e2e_github.sh"

suites=()
api_args=()
browser_file=""
use_webkit=""
skip_webkit=""
pw_args=()

refuse() {
    echo "e2e-github.sh: $1 is not available in GitHub mode. Run with --local instead." >&2
    exit 1
}

for arg in "$@"; do
    [ "$arg" != -- ] || break
    [[ " ${E2E_GITHUB_LOCAL_ONLY_FLAGS[*]} " != *" $arg "* ]] || refuse "$arg"
done

# --commit belongs to the driver, so it comes out before a suite reads its flags.
commit=""
commit_given=""
suite_args=()
while [ $# -gt 0 ]; do
    case "$1" in
        --) suite_args+=("$@"); break ;;
        --commit) commit_given=1; commit="${2:-}"; shift ;;
        --commit=*) commit_given=1; commit="${1#--commit=}" ;;
        *) suite_args+=("$1") ;;
    esac
    shift
done
set -- ${suite_args[@]+"${suite_args[@]}"}
if [ -n "$commit_given" ] && [ -z "$commit" ]; then
    echo "e2e-github.sh: --commit needs a commit" >&2
    exit 1
fi

# The main check has no local fallback and no local leg: whatever stops it
# short of a GitHub verdict is no verdict (scripts/main-check.sh). This is the
# exit for whatever stops a run before it reaches GitHub.
pre_run_exit=1

case "$suite" in
    all)
        suites=(api wasm embedder browser)
        while [ $# -gt 0 ]; do
            case "$1" in
                --no-webkit) skip_webkit=1 ;;
                *) echo "e2e-github.sh: unknown flag for e2e.sh: $1" >&2; exit 1 ;;
            esac
            shift
        done
        ;;
    api)
        suites=(api)
        while [ $# -gt 0 ]; do
            case "$1" in
                --) shift; api_args+=("$@"); break ;;
                *) api_args+=("$1") ;;
            esac
            shift
        done
        ;;
    browser)
        suites=(browser)
        while [ $# -gt 0 ]; do
            case "$1" in
                -f) browser_file="${2:-}"; shift ;;
                --webkit) use_webkit=1 ;;
                --no-webkit) skip_webkit=1 ;;
                --) shift; pw_args+=("$@"); break ;;
                *) pw_args+=("$1") ;;
            esac
            shift
        done
        ;;
    wasm | embedder)
        suites=("$suite")
        [ $# -eq 0 ] || refuse "an argument to $suite"
        ;;
    main-check)
        suites=("${E2E_GITHUB_MAIN_CHECK_SUITES[@]}")
        [ $# -eq 0 ] || refuse "an argument to main-check"
        [ -n "$commit" ] || { echo "e2e-github.sh: main-check tests a named commit; pass --commit <sha>" >&2; exit 1; }
        pre_run_exit="$E2E_GITHUB_NO_VERDICT_EXIT"
        ;;
esac

spec_args=()
if [ "${#pw_args[@]}" -gt 0 ]; then
    rest=()
    prev=""
    for arg in "${pw_args[@]}"; do
        if e2e_github_is_spec_arg "$prev" "$arg"; then
            [[ " ${spec_args[*]-} " == *" $(basename "$arg") "* ]] || spec_args+=("$(basename "$arg")")
        else
            rest+=("$arg")
        fi
        prev="$arg"
    done
    pw_args=(${rest[@]+"${rest[@]}"})
fi
if [ -n "$browser_file" ] && [[ " ${spec_args[*]-} " != *" $(basename "$browser_file") "* ]]; then
    spec_args+=("$(basename "$browser_file")")
fi

if [ -n "$use_webkit" ] && [ -n "$skip_webkit" ]; then
    echo "e2e-github.sh: --webkit and --no-webkit contradict each other" >&2
    exit 1
fi
for arg in ${pw_args[@]+"${pw_args[@]}"}; do
    case "$arg" in
        --project | --project=*) refuse "--project (use --webkit or --no-webkit)" ;;
    esac
done

e2e_github_preflight "$PROJECT_DIR" "$commit" || exit "$pre_run_exit"

# ── Split the browser selection into the remote and local legs ──────────
projects=()
remote_specs=()
local_specs=()
if [[ " ${suites[*]} " == *" browser "* ]]; then
    if [ -n "$use_webkit" ]; then
        projects=(mobile-webkit)
    else
        while IFS= read -r p; do projects+=("$p"); done <<EOF
$(e2e_browser_projects ${skip_webkit:+--no-webkit})
EOF
    fi
    # Under --commit the specs come from that commit, never the working tree.
    routes="$(e2e_github_split_specs "$PROJECT_DIR" "$commit" ${spec_args[@]+"${spec_args[@]}"})" || exit "$pre_run_exit"
    while read -r leg name; do
        case "$leg" in
            local) local_specs+=("$name") ;;
            remote) remote_specs+=("$name") ;;
        esac
    done <<EOF
$routes
EOF
    if [ -n "$commit" ] && [ "${#local_specs[@]}" -gt 0 ]; then
        echo "[e2e-github] testing $commit: ${#local_specs[@]} Claude Code spec(s) need this host and do not run"
        local_specs=()
    fi
fi

run_id="$(e2e_github_run_id)"
branch="$(e2e_github_branch "$run_id")"
out_dir="$PROJECT_DIR/test-results/github/$run_id"
mkdir -p "$out_dir"
request="$out_dir/e2e-request.json"

remote_suites=()
for s in "${suites[@]}"; do
    if [ "$s" = browser ] && [ "${#remote_specs[@]}" -eq 0 ]; then
        continue
    fi
    remote_suites+=("$s")
done

python3 - "$request" "$run_id" "${remote_suites[*]-}" "${projects[*]-}" "${remote_specs[*]-}" \
    -- ${api_args[@]+"${api_args[@]}"} --pw-- ${pw_args[@]+"${pw_args[@]}"} <<'PY'
import json, sys
path, run_id, suites, projects, specs = sys.argv[1:6]
rest = sys.argv[7:]
split = rest.index("--pw--")
json.dump({
    "run_id": run_id,
    "suites": suites.split(),
    "api_args": rest[:split],
    "browser": {"projects": projects.split(), "specs": specs.split(), "pw_args": rest[split + 1:]},
}, open(path, "w"), indent=2)
PY

echo "[e2e-github] run $run_id: remote ${remote_suites[*]:-nothing}; local Claude Code specs: ${#local_specs[@]}"

# ── Cleanup holds on every exit path ─────────────────────────────────────
run_db_id=""
run_done=""
pushed=""
local_pid=""
# shellcheck disable=SC2329 # called by the EXIT trap below
cleanup() {
    local unfinished=""
    [ -n "$run_done" ] || unfinished="$run_db_id"
    e2e_github_stop_run "$PROJECT_DIR" "$branch" "$unfinished" "$pushed"
    if [ -n "$local_pid" ] && kill -0 "$local_pid" 2>/dev/null; then
        kill -TERM "$local_pid" 2>/dev/null
        wait "$local_pid" 2>/dev/null
    fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

e2e_github_sweep "$PROJECT_DIR"

# ── The local leg ────────────────────────────────────────────────────────
local_log="$out_dir/local-claude-code.log"

# The local leg runs beside a long remote run, so it waits for this host's e2e
# lock rather than failing on it. A refusal is retried, because another run can
# take the lock between the check and the start.
# shellcheck disable=SC2329 # run in the background below
run_local_leg() {
    local waited=0 limit="${E2E_GITHUB_LOCK_WAIT_SECS:-3600}" holder child rc
    trap '[ -z "${child:-}" ] || kill -TERM "$child" 2>/dev/null; exit 143' TERM
    export LUCIDOS_E2E_LOCAL=1
    while :; do
        holder="$(e2e_github_e2e_lock_holder)"
        if [ -n "$holder" ]; then
            if [ "$waited" -ge "$limit" ]; then
                echo "ERROR: the e2e lock stayed held by $holder for ${limit}s; the local leg did not run." >"$local_log"
                return "$E2E_GITHUB_NO_VERDICT_EXIT"
            fi
            [ "$waited" -gt 0 ] || echo "[e2e-github] local leg waits for the e2e lock, held by $holder"
            sleep 30
            waited=$((waited + 30))
            continue
        fi
        "$SCRIPT_DIR/e2e-browser.sh" ${local_flags[@]+"${local_flags[@]}"} \
            "${local_selection[@]}" ${pw_args[@]+"${pw_args[@]}"} >"$local_log" 2>&1 &
        child=$!
        wait "$child"
        rc=$?
        child=""
        if [ "$rc" -ne 0 ] && [ "$waited" -lt "$limit" ] \
            && grep -qF "$E2E_LOCK_HELD_MESSAGE" "$local_log"; then
            sleep 30
            waited=$((waited + 30))
            continue
        fi
        return "$rc"
    done
}
if [ "${#local_specs[@]}" -gt 0 ]; then
    # A single spec goes through -f, so projects that ignore it are skipped
    # exactly as in a local run.
    local_selection=()
    if [ -n "$browser_file" ] && [ "${#spec_args[@]}" -eq 1 ]; then
        local_selection=(-f "$(basename "$browser_file")" --)
    else
        local_selection=(--)
        for s in "${local_specs[@]}"; do local_selection+=("$(playwright_file_filter "$s")"); done
    fi
    local_flags=()
    [ -z "$use_webkit" ] || local_flags+=(--webkit)
    [ -z "$skip_webkit" ] || local_flags+=(--no-webkit)
    echo "[e2e-github] local leg: ${#local_specs[@]} Claude Code spec(s) on this host, log at $local_log"
    run_local_leg &
    local_pid=$!
fi

# ── The remote leg ───────────────────────────────────────────────────────
remote_rc=0
if [ "${#remote_suites[@]}" -gt 0 ]; then
    pushed_commit="$(e2e_github_build_commit "$PROJECT_DIR" "$request" "$run_id" "${commit:-HEAD}")" || {
        echo "ERROR: nothing was pushed." >&2
        exit "$E2E_GITHUB_NO_VERDICT_EXIT"
    }
    e2e_github_push "$PROJECT_DIR" "$pushed_commit" "$branch" || {
        echo "ERROR: the push to $E2E_GITHUB_REMOTE/$branch was refused." >&2
        exit "$E2E_GITHUB_NO_VERDICT_EXIT"
    }
    pushed=1
    run_db_id="$(e2e_github_find_run "$branch" "$pushed_commit")" || exit "$E2E_GITHUB_NO_VERDICT_EXIT"
    # Under pipefail a wait that gave up fails the pipeline, though tail ends 0.
    # The EXIT trap then cancels the run.
    conclusion="$(e2e_github_wait "$run_db_id" | tail -1)" || exit "$E2E_GITHUB_NO_VERDICT_EXIT"
    run_done=1
    echo "[e2e-github] run concluded: ${conclusion:-unknown}"
    if ! e2e_github_download "$run_db_id" "$out_dir"; then
        echo "ERROR: could not download the run's output." >&2
        remote_rc="$E2E_GITHUB_NO_VERDICT_EXIT"
    else
        e2e_github_report "$out_dir"
        remote_rc=$?
    fi
fi

# ── Merge the local leg ──────────────────────────────────────────────────
overall="$remote_rc"
if [ -n "$local_pid" ]; then
    wait "$local_pid"
    local_rc=$?
    local_pid=""
    echo ""
    echo "══ browser: Claude Code specs (local leg) ══"
    cat "$local_log"
    echo "── local leg exit code: $local_rc ──"
    overall="$(e2e_github_worse_rc "$overall" "$local_rc")"
fi

echo ""
echo "[e2e-github] output kept under ${out_dir#"$PROJECT_DIR"/}"
exit "$overall"
