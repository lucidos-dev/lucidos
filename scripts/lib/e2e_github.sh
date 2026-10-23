#!/usr/bin/env bash
# e2e_github.sh: the driver half of e2e GitHub mode (ADR 0382).
#
# The driver strips HEAD, or the commit a caller names, with release_tree. It
# scans the result and commits it with no parent. It pushes that commit to an
# e2e/<run-id> branch of the mirror, where .github/workflows/e2e.yml runs the
# shards. It then waits, downloads each shard's output, and reports it through
# the tally a local run uses.
#
# Two rules bind every change here:
#   - Nothing reaches the mirror except through release_tree_push, which scans
#     the exact tree being pushed. The commit has no parent (ADR 0039, 0462).
#   - Only refs under e2e/ are ever deleted.
#
# Sourced by scripts/e2e-github.sh. Tested by scripts/lib/e2e_github_test.sh.

# shellcheck source=scripts/lib/touched_specs.sh
source "$(dirname "${BASH_SOURCE[0]}")/touched_specs.sh"
# shellcheck source=scripts/lib/e2e_warm.sh
source "$(dirname "${BASH_SOURCE[0]}")/e2e_warm.sh"

E2E_GITHUB_REPO="lucidos-dev/lucidos"
E2E_GITHUB_REMOTE="lucidos"
E2E_GITHUB_WORKFLOW="e2e.yml"
E2E_GITHUB_BRANCH_PREFIX="e2e/"
E2E_GITHUB_REQUEST_PATH=".github/e2e-request.json"
E2E_GITHUB_AUTHOR_NAME="Lucidos e2e"
E2E_GITHUB_AUTHOR_EMAIL="lucidos-e2e@users.noreply.github.com"
E2E_GITHUB_SWEEP_AGE_SECS=86400
# The line naming a run once it is found: "<mark><run url> <branch>". The main
# check reads it, and needs both to reattach after an engine restart.
# shellcheck disable=SC2034 # read by scripts/e2e-github.sh and scripts/lib/main_check.sh
E2E_GITHUB_LINKED_MARK="[e2e-github] linked "
# A run that never produced a verdict: a refused push, a workflow that never
# started, a cancelled run, a shard with no output. Distinct from 71, 72, 75.
E2E_GITHUB_NO_VERDICT_EXIT=76
# What the main check runs: the engine suite, then every e2e suite. Only the
# main check asks for the engine suite (ADR 0386's limits, as amended).
# shellcheck disable=SC2034 # read by scripts/e2e-github.sh after it sources this
E2E_GITHUB_MAIN_CHECK_SUITES=(engine api wasm embedder browser)

e2e_github_now() { date +%s; }

# e2e_github_is_spec_arg <previous-arg> <arg>: whether a Playwright argument
# names a spec file. A local run hands it to Playwright as a file filter, so
# GitHub mode narrows both legs to it rather than forwarding it. A value of an
# option that takes one is never a spec, whatever it ends in.
e2e_github_is_spec_arg() {
    case "$1" in
        -g | --grep | --grep-invert | -c | --config | --output | --reporter | --shard | -j | --workers) return 1 ;;
    esac
    case "$2" in
        -*) return 1 ;;
        *.spec.ts) return 0 ;;
        *) return 1 ;;
    esac
}

# Flags that only mean something on this host, so they keep a run here.
E2E_GITHUB_LOCAL_ONLY_FLAGS=(--no-reset -h --headed --ios --device --screenshot --pwa --packaged)

# Options whose next argument is their value, never a filter: the e2e scripts'
# own before `--`, Playwright's and cargo test's after it.
E2E_GITHUB_VALUE_FLAGS=(-f --device --commit --attach --branch)
E2E_GITHUB_TAIL_VALUE_FLAGS=(-c --config --output --reporter --shard -j --workers --project
    --repeat-each --retries --timeout --max-failures --skip --test-threads --format --color)

# e2e_github_is_filtered <arg>...: whether a run selects a subset of its suite.
# Filtered means -f, a positional argument, or after `--` a Playwright --grep,
# -g or --grep-invert, or any positional. Filtered runs stay on this host.
e2e_github_is_filtered() {
    local arg past_dashes="" skip=""
    for arg in "$@"; do
        if [ -n "$skip" ]; then
            skip=""
            continue
        fi
        if [ -z "$past_dashes" ]; then
            case "$arg" in
                --) past_dashes=1 ;;
                -f) return 0 ;;
                -*) [[ " ${E2E_GITHUB_VALUE_FLAGS[*]} " != *" $arg "* ]] || skip=1 ;;
                *) return 0 ;;
            esac
            continue
        fi
        case "$arg" in
            -g | --grep | --grep=* | --grep-invert | --grep-invert=*) return 0 ;;
            -*) [[ " ${E2E_GITHUB_TAIL_VALUE_FLAGS[*]} " != *" $arg "* ]] || skip=1 ;;
            *) return 0 ;;
        esac
    done
    return 1
}

# e2e_github_lean_local <suite>: shape a filtered run that stays here, editing
# E2E_ARGS. It builds a debug engine, which compiles incrementally, and skips
# mobile-webkit. `--webkit`, or a `--project` for Playwright, keeps WebKit and
# the release engine. An explicit LUCIDOS_E2E_DEBUG or RELEASE wins.
e2e_github_lean_local() {
    local suite="$1" arg
    for arg in ${E2E_ARGS[@]+"${E2E_ARGS[@]}"}; do
        case "$arg" in
            --webkit | --project | --project=*) return 0 ;;
        esac
    done
    if [ -z "${LUCIDOS_E2E_DEBUG+x}" ] && [ -z "${RELEASE+x}" ]; then
        export LUCIDOS_E2E_DEBUG=1
    fi
    if [ "$suite" = browser ] && [[ " ${E2E_ARGS[*]} " != *" --no-webkit "* ]]; then
        E2E_ARGS=(--no-webkit "${E2E_ARGS[@]}")
        echo "[e2e] mobile-webkit is skipped on this host. Pass --webkit to run it." >&2
    fi
}

# e2e_github_local_only_env: name the first set environment variable that
# changes what a run tests or produces. The driver forwards no environment, so
# GitHub would silently ignore it. Any `*_SHOTS` switch counts.
e2e_github_local_only_env() {
    local name
    for name in $(compgen -e); do
        case "$name" in
            *_SHOTS | LUCIDOS_E2E_PACKAGED | LUCIDOS_E2E_DEBUG | LUCIDOS_E2E_WEBKIT_PHASE | LUCIDOS_E2E_WEBKIT_CHUNKS) ;;
            *) continue ;;
        esac
        case "${!name}" in
            '' | 0) ;;
            *) echo "$name"; return 0 ;;
        esac
    done
}

# e2e_github_handoff <suite> "$@": GitHub is the default for a whole suite.
# Replace this process with the driver unless the run must stay here: --local,
# a filtered run, a local-only flag or variable, an inherited
# LUCIDOS_E2E_LOCAL, or an unmet precondition. Only flags before any `--`
# count, except for the filter test. Staying here exports LUCIDOS_E2E_LOCAL=1,
# so every nested e2e script stays too. Each e2e script calls this first,
# before the e2e lock, and then parses E2E_ARGS: its arguments minus --local
# and --github, plus what e2e_github_lean_local adds.
e2e_github_handoff() {
    local suite="$1" arg github="" local_run="" past_dashes="" reason="" env_name lean=""
    shift
    E2E_ARGS=()
    for arg in "$@"; do
        if [ -z "$past_dashes" ]; then
            case "$arg" in
                --github) github=1; continue ;;
                --local) local_run=1; continue ;;
                --) past_dashes=1 ;;
                *) [[ " ${E2E_GITHUB_LOCAL_ONLY_FLAGS[*]} " != *" $arg "* ]] || local_run=1 ;;
            esac
        fi
        E2E_ARGS+=("$arg")
    done
    # GitHub mode passes no cargo arguments to wasm or embedder.
    case "$suite" in
        wasm | embedder) [ -z "${E2E_ARGS[0]+x}" ] || local_run=1 ;;
    esac
    env_name="$(e2e_github_local_only_env)"
    if [ -n "$env_name" ] && [ -z "$local_run" ] && [ -z "$github" ] && [ -z "${LUCIDOS_E2E_LOCAL:-}" ]; then
        echo "[e2e] running on this host: $env_name is set, and GitHub mode does not forward it." >&2
    fi
    [ -z "$env_name" ] || local_run=1
    if [ -n "$github" ] && [ -n "$local_run" ] && [ -z "${LUCIDOS_E2E_LOCAL:-}" ]; then
        echo "ERROR: --github contradicts --local, or a flag or variable that only works on this host." >&2
        exit 1
    fi
    if [ "${LUCIDOS_E2E_LOCAL:-}" = 1 ]; then
        local_run=1
    elif [ -z "$github" ] && e2e_github_is_filtered ${E2E_ARGS[@]+"${E2E_ARGS[@]}"}; then
        [ -n "$local_run" ] || echo "[e2e] running on this host: a filtered run. Pass --github to run it on GitHub." >&2
        local_run=1
        lean=1
    elif [ -z "$local_run" ] && [ -z "$github" ]; then
        reason="$(e2e_github_unavailable_reason "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)")"
        if [ -n "$reason" ]; then
            echo "[e2e] running on this host: $reason." >&2
            local_run=1
        fi
    fi
    if [ -n "$local_run" ]; then
        [ -z "$lean" ] || e2e_github_lean_local "$suite"
        export LUCIDOS_E2E_LOCAL=1
        return 0
    fi
    echo "[e2e] running on GitHub (ADR 0386). Pass --local to run on this host." >&2
    exec "$(dirname "${BASH_SOURCE[0]}")/../e2e-github.sh" "$suite" ${E2E_ARGS[@]+"${E2E_ARGS[@]}"}
}

# A run id sorts by time and carries its own start, so the sweep can age a
# branch without asking the API.
e2e_github_run_id() {
    printf '%s-%s\n' "$(date -u +%Y%m%d-%H%M%S)" "$(od -An -N2 -tx1 /dev/urandom | tr -d ' \n')"
}

e2e_github_branch() { printf '%s%s\n' "$E2E_GITHUB_BRANCH_PREFIX" "$1"; }

e2e_github_is_run_branch() {
    case "$1" in
        "$E2E_GITHUB_BRANCH_PREFIX"*) [[ "${1#"$E2E_GITHUB_BRANCH_PREFIX"}" =~ ^[0-9]{8}-[0-9]{6}-[0-9a-f]+$ ]] ;;
        *) return 1 ;;
    esac
}

# e2e_github_branch_started <branch>: the epoch second its run id names.
e2e_github_branch_started() {
    local stamp="${1#"$E2E_GITHUB_BRANCH_PREFIX"}"
    python3 -c 'import calendar, sys, time; print(calendar.timegm(time.strptime(sys.argv[1], "%Y%m%d-%H%M%S")))' \
        "${stamp:0:15}"
}

# e2e_github_unavailable_reason <repo> [<commit>]: print why GitHub mode cannot
# test <repo> honestly, or nothing when it can. A named commit is tested as it
# was committed, so uncommitted changes beside it do not matter.
e2e_github_unavailable_reason() {
    local repo="$1" commit="${2:-}"
    if [ ! -f "$repo/scripts/lib/release_tree.sh" ]; then
        echo "this checkout has no scripts/lib/release_tree.sh, which the public mirror does not ship"
    elif [ -n "$commit" ] && ! git -C "$repo" rev-parse --verify --quiet "$commit^{commit}" >/dev/null; then
        echo "'$commit' names no commit in this repository"
    elif [ -z "$commit" ] && [ -n "$(git -C "$repo" status --porcelain)" ]; then
        echo "the tree has uncommitted changes, and GitHub mode tests the committed HEAD"
    elif ! git -C "$repo" remote get-url "$E2E_GITHUB_REMOTE" >/dev/null 2>&1; then
        echo "there is no '$E2E_GITHUB_REMOTE' remote for $E2E_GITHUB_REPO"
    elif ! command -v gh >/dev/null 2>&1; then
        echo "the gh CLI is not installed"
    fi
}

# e2e_github_preflight <repo> [<commit>]: refuse what GitHub mode cannot test
# honestly.
e2e_github_preflight() {
    local reason
    reason="$(e2e_github_unavailable_reason "$1" "${2:-}")"
    [ -n "$reason" ] || return 0
    echo "ERROR: GitHub mode cannot run: $reason." >&2
    echo "       Fix that, or run the suite with --local." >&2
    return 1
}

# The browser specs, relative to the repository root.
E2E_GITHUB_SPEC_DIR="crates/lucidos-app/e2e"

# e2e_github_split_specs <repo> <commit> [<spec>...]: route each browser spec.
# Prints "local <name>" for a spec with a @real-claude-code test and "remote
# <name>" for any other. With no <spec>, every spec. A <commit> reads the specs
# from that commit, the tree GitHub tests. An empty one reads the working
# tree. Fails naming a spec that is not there. Needs scripts/lib/e2e.sh.
e2e_github_split_specs() {
    local repo="$1" commit="$2" name path spec names=()
    shift 2
    if [ "$#" -gt 0 ]; then
        names=("$@")
    elif [ -n "$commit" ]; then
        while IFS= read -r name; do
            case "$name" in *.spec.ts) names+=("$name") ;; esac
        done <<EOF
$(git -C "$repo" ls-tree --name-only "$commit:$E2E_GITHUB_SPEC_DIR" 2>/dev/null || true)
EOF
    else
        for path in "$repo/$E2E_GITHUB_SPEC_DIR"/*.spec.ts; do
            [ -f "$path" ] && names+=("$(basename "$path")")
        done
    fi
    for name in ${names[@]+"${names[@]}"}; do
        path="$E2E_GITHUB_SPEC_DIR/$name"
        if [ -n "$commit" ]; then
            git -C "$repo" cat-file -e "$commit:$path" 2>/dev/null \
                || { echo "e2e-github.sh: no spec named $name in $commit" >&2; return 1; }
            # Read whole before the check: grep -q stops at the first match, and
            # a git still writing into that pipe dies of SIGPIPE.
            spec="$(git -C "$repo" show "$commit:$path")" \
                || { echo "e2e-github.sh: could not read $name from $commit" >&2; return 1; }
            if e2e_spec_needs_real_claude_code - <<<"$spec"; then
                echo "local $name"
            else
                echo "remote $name"
            fi
        else
            [ -f "$repo/$path" ] || { echo "e2e-github.sh: no spec named $name" >&2; return 1; }
            if e2e_spec_needs_real_claude_code "$repo/$path"; then
                echo "local $name"
            else
                echo "remote $name"
            fi
        fi
    done
}

# e2e_github_build_commit <repo> <request-file> <run-id> [<commit>]: print the
# commit to push, built from <commit> (default HEAD). Fails, having pushed
# nothing, when the scan refuses the tree.
e2e_github_build_commit() {
    local repo="$1" request="$2" run_id="$3" source="${4:-HEAD}"
    local tree blob index full_tree=""

    tree="$(release_tree_build "$repo" "$source")" || return 1
    blob="$(git -C "$repo" hash-object -w "$request")" || return 1
    index="$(mktemp "${TMPDIR:-/tmp}/lucidos-e2e-index.XXXXXX")" || return 1
    if GIT_INDEX_FILE="$index" git -C "$repo" read-tree "$tree" \
        && GIT_INDEX_FILE="$index" git -C "$repo" update-index --add \
            --cacheinfo "100644,$blob,$E2E_GITHUB_REQUEST_PATH"; then
        full_tree="$(GIT_INDEX_FILE="$index" git -C "$repo" write-tree)" || full_tree=""
    fi
    rm -f "$index"
    [ -n "$full_tree" ] || { echo "ERROR: could not add the e2e request to the stripped tree" >&2; return 1; }

    release_tree_scan "$repo" "$full_tree" || return 1

    GIT_AUTHOR_NAME="$E2E_GITHUB_AUTHOR_NAME" GIT_AUTHOR_EMAIL="$E2E_GITHUB_AUTHOR_EMAIL" \
    GIT_COMMITTER_NAME="$E2E_GITHUB_AUTHOR_NAME" GIT_COMMITTER_EMAIL="$E2E_GITHUB_AUTHOR_EMAIL" \
        git -C "$repo" commit-tree "$full_tree" -m "e2e run $run_id"
}

e2e_github_push() {
    local repo="$1" commit="$2" branch="$3"
    e2e_github_is_run_branch "$branch" || { echo "ERROR: refusing to push to '$branch'" >&2; return 1; }
    local out
    # GitHub answers every new branch with a pull-request hint; show it only
    # when the push failed.
    out="$(release_tree_push "$repo" "$E2E_GITHUB_REMOTE" "$commit" "refs/heads/$branch" 2>&1)" \
        || { printf '%s\n' "$out" >&2; return 1; }
}

e2e_github_delete_branch() {
    local repo="$1" branch="$2"
    e2e_github_is_run_branch "$branch" || { echo "ERROR: refusing to delete '$branch'" >&2; return 1; }
    release_tree_delete_ref "$repo" "$E2E_GITHUB_REMOTE" "refs/heads/$branch"
}

# e2e_github_unfinished_runs <branch>: how many of the branch's runs have not
# completed. Prints nothing when the API cannot answer.
e2e_github_unfinished_runs() {
    gh run list --repo "$E2E_GITHUB_REPO" --branch "$1" --limit 20 \
        --json status --jq 'map(select(.status != "completed")) | length' 2>/dev/null
}

# e2e_github_sweep <repo>: delete e2e run branches over a day old whose runs have
# finished. A branch the API cannot vouch for is kept.
e2e_github_sweep() {
    local repo="$1" now ref branch started unfinished
    now="$(e2e_github_now)"
    while read -r _ ref; do
        [ -n "$ref" ] || continue
        branch="${ref#refs/heads/}"
        e2e_github_is_run_branch "$branch" || continue
        started="$(e2e_github_branch_started "$branch")" || continue
        [ $((now - started)) -gt "$E2E_GITHUB_SWEEP_AGE_SECS" ] || continue
        unfinished="$(e2e_github_unfinished_runs "$branch")"
        [ "$unfinished" = "0" ] || continue
        echo "[e2e-github] sweeping finished e2e run branch $branch"
        e2e_github_delete_branch "$repo" "$branch" || true
    done <<EOF
$(release_mirror_ls_remote "$repo" "$E2E_GITHUB_REMOTE" "refs/heads/$E2E_GITHUB_BRANCH_PREFIX*" 2>/dev/null)
EOF
}

# e2e_github_find_run <branch> <commit>: print the workflow run the push
# started. Waits up to E2E_GITHUB_START_WAIT_SECS (default 180).
e2e_github_find_run() {
    local branch="$1" commit="$2" waited=0 limit="${E2E_GITHUB_START_WAIT_SECS:-180}" id
    while [ "$waited" -lt "$limit" ]; do
        id="$(gh run list --repo "$E2E_GITHUB_REPO" --workflow "$E2E_GITHUB_WORKFLOW" \
            --branch "$branch" --limit 5 --json databaseId,headSha \
            --jq ".[] | select(.headSha == \"$commit\") | .databaseId" 2>/dev/null | head -1)"
        if [ -n "$id" ]; then
            printf '%s\n' "$id"
            return 0
        fi
        sleep 5
        waited=$((waited + 5))
    done
    echo "ERROR: no $E2E_GITHUB_WORKFLOW run started for $branch within ${limit}s" >&2
    return 1
}

# e2e_github_run_progress <run-json>: one line naming where the jobs stand.
e2e_github_run_progress() {
    python3 -c '
import json, sys
run = json.loads(sys.argv[1])
jobs = run.get("jobs") or []
def count(*states):
    return sum(1 for j in jobs if j.get("status") in states)
failed = sum(1 for j in jobs if j.get("conclusion") in ("failure", "cancelled", "timed_out"))
print("run %s: %d queued, %d running, %d/%d jobs done, %d failed" % (
    run.get("status"), count("queued", "waiting", "pending"), count("in_progress"),
    count("completed"), len(jobs), failed))
' "$1"
}

e2e_github_run_url() { printf 'https://github.com/%s/actions/runs/%s\n' "$E2E_GITHUB_REPO" "$1"; }

# e2e_github_run_state <run-id>: one JSON object with the run's status,
# conclusion and jobs, plus `rate_remaining`, the lower X-RateLimit-Remaining
# of the two answers. Fails when either call fails.
e2e_github_run_state() {
    local run jobs
    run="$(gh api --include "repos/$E2E_GITHUB_REPO/actions/runs/$1" 2>/dev/null)" || return 1
    jobs="$(gh api --include "repos/$E2E_GITHUB_REPO/actions/runs/$1/jobs?per_page=100" 2>/dev/null)" || return 1
    python3 -c '
import json, sys

def split(raw):
    head, _, body = raw.replace("\r\n", "\n").partition("\n\n")
    headers = {}
    for line in head.split("\n")[1:]:
        name, _, value = line.partition(":")
        headers[name.strip().lower()] = value.strip()
    return headers, json.loads(body)

run_headers, run = split(sys.argv[1])
jobs_headers, jobs = split(sys.argv[2])
remaining = [int(h["x-ratelimit-remaining"]) for h in (run_headers, jobs_headers)
             if h.get("x-ratelimit-remaining", "").isdigit()]
keep = ("name", "status", "conclusion", "created_at", "started_at", "completed_at")
print(json.dumps({
    "status": run.get("status"),
    "conclusion": run.get("conclusion"),
    "updated_at": run.get("updated_at"),
    "jobs": [{k: j.get(k) for k in keep} for j in jobs.get("jobs") or []],
    "rate_remaining": min(remaining) if remaining else None,
}))
' "$run" "$jobs"
}

# Poll intervals in seconds: before the test jobs start, once they have, and
# the rate-limit floor that sends a wait back to the slow interval.
E2E_GITHUB_POLL_SLOW_SECS=30
E2E_GITHUB_POLL_FAST_SECS=5
E2E_GITHUB_RATE_FLOOR=500

# e2e_github_poll_secs <run-state>: how long to sleep before the next poll. A
# test job is any job but plan and the build-* jobs.
e2e_github_poll_secs() {
    python3 -c '
import json, sys
state, slow, fast, floor = json.loads(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
remaining = state.get("rate_remaining")
started = any(j.get("status") in ("in_progress", "completed")
              for j in state.get("jobs") or []
              if j.get("name") != "plan" and not (j.get("name") or "").startswith("build-"))
print(fast if started and (remaining is None or remaining >= floor) else slow)
' "$1" "$E2E_GITHUB_POLL_SLOW_SECS" "$E2E_GITHUB_POLL_FAST_SECS" "$E2E_GITHUB_RATE_FLOOR"
}

# e2e_github_wait <run-id>: block until the run completes, printing progress
# whenever it changes. Prints the final run state as JSON on the last line of
# stdout. Polls adaptively (e2e_github_poll_secs), and a failed gh call waits
# the slow interval. Gives up after E2E_GITHUB_RUN_WAIT_SECS (default 6 hours)
# with the no-verdict exit. The default sits above the longest chain of
# timeout-minutes in e2e.yml, with room for queueing.
e2e_github_wait() {
    local run_id="$1" limit="${E2E_GITHUB_RUN_WAIT_SECS:-21600}"
    local state status line last="" deadline poll
    deadline=$(($(e2e_github_now) + limit))
    echo "[e2e-github] watching $(e2e_github_run_url "$run_id")" >&2
    while :; do
        poll="$E2E_GITHUB_POLL_SLOW_SECS"
        if state="$(e2e_github_run_state "$run_id")"; then
            line="$(e2e_github_run_progress "$state")"
            [ "$line" = "$last" ] || { echo "[e2e-github] $line" >&2; last="$line"; }
            status="$(printf '%s' "$state" | python3 -c 'import json, sys; print(json.load(sys.stdin)["status"])')"
            if [ "$status" = "completed" ]; then
                printf '%s\n' "$state"
                return 0
            fi
            poll="$(e2e_github_poll_secs "$state")"
        fi
        if [ "$(e2e_github_now)" -ge "$deadline" ]; then
            echo "ERROR: run $run_id did not complete within ${limit}s, so it produced no verdict." >&2
            return "$E2E_GITHUB_NO_VERDICT_EXIT"
        fi
        sleep "$poll"
    done
}

# e2e_github_conclusion <run-state>: the run's conclusion, or nothing.
e2e_github_conclusion() {
    printf '%s' "$1" | python3 -c 'import json, sys; print(json.load(sys.stdin).get("conclusion") or "")'
}

# e2e_github_timings_log: the one log every run appends its timing record to,
# a JSON line each, shared by every worktree on this host.
e2e_github_timings_log() {
    printf '%s\n' "${LUCIDOS_E2E_TIMINGS_LOG:-$HOME/.lucidos/e2e-github/timings.jsonl}"
}

# e2e_github_record_timings <out-dir> <run-db-id> <suite> <pushed-at> <rc> <run-state>:
# write <out-dir>/timings.json, append it to the timings log, and print one
# summary line. <pushed-at> is the epoch second of the push, and the verdict is
# now. The plan's engines file names, per platform, the run whose engine a test
# job reused, or "-" for one this run built. A record that cannot be written
# is one line and never changes the verdict.
e2e_github_record_timings() {
    local dir="$1" run_db_id="$2" suite="$3" pushed_at="$4" rc="$5" state="$6" log
    log="$(e2e_github_timings_log)"
    mkdir -p "$(dirname "$log")" 2>/dev/null
    python3 -I - "$dir" "$run_db_id" "$suite" "$pushed_at" "$(e2e_github_now)" "$rc" "$state" \
        "$(e2e_github_run_url "$run_db_id")" "$log" <<'PY' || echo "[e2e-github] timings not recorded" >&2
import json, os, sys
from datetime import datetime, timezone

dir, run_db_id, suite, pushed_at, now, rc, state, url, log = sys.argv[1:10]
state, pushed_at, now = json.loads(state), int(pushed_at), int(now)


def epoch(stamp):
    return int(datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp()) if stamp else None


def span(a, b):
    return b - a if a is not None and b is not None else None


def clock(secs):
    return "?" if secs is None else f"{secs // 60}m{secs % 60:02d}s"


engines = {}
try:
    for line in open(os.path.join(dir, "e2e-plan", "engines")):
        parts = line.split()
        if len(parts) == 2:
            engines[parts[0]] = None if parts[1] == "-" else parts[1]
except OSError:
    pass
jobs = [{
    "name": j.get("name"),
    "conclusion": j.get("conclusion"),
    "queue_secs": span(epoch(j.get("created_at")), epoch(j.get("started_at"))),
    "run_secs": span(epoch(j.get("started_at")), epoch(j.get("completed_at"))),
} for j in state.get("jobs") or [] if j.get("conclusion") != "skipped"]
record = {
    "run": url,
    "run_id": int(run_db_id),
    "suite": suite,
    "rc": int(rc),
    "pushed_at": datetime.fromtimestamp(pushed_at, timezone.utc).isoformat(),
    "push_to_complete_secs": span(pushed_at, epoch(state.get("updated_at"))),
    "push_to_verdict_secs": now - pushed_at,
    "engine_reused_from": engines,
    "jobs": jobs,
}
with open(os.path.join(dir, "timings.json"), "w") as f:
    json.dump(record, f, indent=2)
with open(log, "a") as f:
    f.write(json.dumps(record) + "\n")
reuse = "; ".join(f"{os_} engine {'reused from run ' + run if run else 'built'}"
                  for os_, run in sorted(engines.items()))
slowest = max((j for j in jobs if j["run_secs"] is not None), key=lambda j: j["run_secs"], default=None)
parts = [f"push to verdict {clock(record['push_to_verdict_secs'])}",
         f"run finished at {clock(record['push_to_complete_secs'])}"]
if reuse:
    parts.append(reuse)
if slowest:
    parts.append(f"slowest job {slowest['name']} {clock(slowest['run_secs'])}")
print("[e2e-github] timings: " + ", ".join(parts), file=sys.stderr)
PY
}

e2e_github_cancel() {
    gh run cancel "$1" --repo "$E2E_GITHUB_REPO" >/dev/null 2>&1 || true
}

# e2e_github_stop_run <repo> <branch> <unfinished-run-id> <pushed> [<detached>]:
# on the way out, cancel a run that has not finished and delete the branch once
# pushed. A detached driver leaves both for a later --attach. It often runs
# after a signal that took the reader of stderr too, so it ignores SIGPIPE: a
# broken pipe must not stop it before either call.
e2e_github_stop_run() {
    local repo="$1" branch="$2" run_db_id="$3" pushed="$4" detached="${5:-}"
    trap '' PIPE
    if [ -n "$detached" ]; then
        echo "[e2e-github] detached: run ${run_db_id:-(finished)} and $branch stay for --attach" >&2
        return 0
    fi
    if [ -n "$run_db_id" ]; then
        echo "[e2e-github] cancelling run $run_db_id" >&2
        e2e_github_cancel "$run_db_id"
    fi
    if [ -n "$pushed" ]; then
        e2e_github_delete_branch "$repo" "$branch" || echo "[e2e-github] could not delete $branch; the next run's sweep will" >&2
    fi
}

# e2e_github_trap_detach: SIGUSR2 ends the driver with no verdict. Once the run
# is found ($run_db_id) it sets $detached, so cleanup leaves the run going.
# Before that there is nothing to leave going, so the cleanup cancels.
e2e_github_trap_detach() {
    trap '[ -z "$run_db_id" ] || detached=1; exit "$E2E_GITHUB_NO_VERDICT_EXIT"' USR2
}

# A detached driver may have downloaded part of the run already, and gh fails
# rather than extract over a file it finds. So each download starts empty.
e2e_github_download() {
    local run_id="$1" dir="$2"
    mkdir -p "$dir"
    rm -rf "$dir"/e2e-out-* "$dir"/e2e-plan
    gh run download "$run_id" --repo "$E2E_GITHUB_REPO" -D "$dir" -p 'e2e-out-*' -p 'e2e-plan'
}

# e2e_github_record_coverage_map <root> <out-dir> <commit> <run-url>: merge the
# Chromium shards' coverage files into the coverage map (ADR 0502, as amended).
# Replaces the map atomically, and only when every Chromium shard reported. A
# refusal is one line and never changes the run's verdict.
e2e_github_record_coverage_map() {
    local root="$1" dir="$2" commit="$3" run_url="$4" map
    map="$(ts_coverage_map_path "$root")" || {
        echo "[e2e-github] coverage map not updated: no git dir under $root" >&2
        return 0
    }
    python3 -I - "$dir" "$map" "$commit" "$run_url" <<'PY' || true
import glob, json, os, sys, tempfile

dir, map_path, commit, run_url = sys.argv[1:5]
CHROMIUM = {"chromium", "mobile"}


def refuse(why):
    print(f"[e2e-github] coverage map not updated: {why}", file=sys.stderr)
    sys.exit(0)


try:
    entries = [line.split() for line in open(os.path.join(dir, "e2e-plan", "entries"))]
except OSError:
    refuse("the run has no shard plan")
shards = [e[0] for e in entries if len(e) == 5 and e[1] == "browser" and e[2] in CHROMIUM]
if not shards:
    refuse("the run had no Chromium shard")
specs, missing = {}, []
for shard in shards:
    files = glob.glob(os.path.join(dir, f"e2e-out-{shard}", "coverage", "*.json"))
    if not files:
        missing.append(shard)
    for path in files:
        try:
            record = json.load(open(path))
            specs.setdefault(record["spec"], set()).update(record["files"])
        except (OSError, ValueError, KeyError, TypeError):
            refuse(f"{os.path.relpath(path, dir)} is unreadable")
if missing:
    refuse(f"{len(missing)} Chromium shard(s) recorded no coverage: {' '.join(missing)}")
os.makedirs(os.path.dirname(map_path), exist_ok=True)
fd, tmp = tempfile.mkstemp(dir=os.path.dirname(map_path), prefix=".e2e-coverage-map.")
with os.fdopen(fd, "w") as f:
    json.dump({"commit": commit, "run": run_url,
               "specs": {k: sorted(v) for k, v in sorted(specs.items())}}, f)
os.replace(tmp, map_path)
print(f"[e2e-github] coverage map updated: {len(specs)} specs, from {commit}", file=sys.stderr)
PY
}

# ── The report ───────────────────────────────────────────────────────────
#
# The plan artifact lists one line per shard: "<id> <suite> <project> <shard>
# <total>". Each shard uploads e2e-out-<id>/<id>.log and <id>.rc. A shard the
# plan names but whose output is missing has no verdict. So does a shard whose
# build the memory gate refused, or whose warm-up failed, since it tested
# nothing.

# e2e_github_shard_rc <dir> <id>: the shard's exit code, or the no-verdict code.
# Needs HOST_MEMORY_BUILD_REFUSED_EXIT, which scripts/lib/e2e.sh sources.
e2e_github_shard_rc() {
    local rc_file="$1/e2e-out-$2/$2.rc" rc task
    rc="$(cat "$rc_file" 2>/dev/null)"
    case "$rc" in
        '' | *[!0-9]*) echo "$E2E_GITHUB_NO_VERDICT_EXIT" ;;
        "$HOST_MEMORY_BUILD_REFUSED_EXIT")
            echo "ERROR: the $2 shard's build never started (exit $rc), so it tested nothing." >&2
            echo "$E2E_GITHUB_NO_VERDICT_EXIT"
            ;;
        "$E2E_WARM_FAILED_EXIT")
            task="$(sed -n 's/^ERROR: warm-up task \([^ ]*\) .*/\1/p' "$1/e2e-out-$2/$2.log" 2>/dev/null | tail -n 1)"
            echo "ERROR: the $2 shard's ${task:-warm-up} setup failed (exit $rc), so it tested nothing." >&2
            echo "$E2E_GITHUB_NO_VERDICT_EXIT"
            ;;
        *) echo "$rc" ;;
    esac
}

e2e_github_shard_log() { printf '%s\n' "$1/e2e-out-$2/$2.log"; }

# e2e_github_worse_rc <so-far> <rc>: the exit code a run reports so far. The
# first failure wins, and any failure outranks no verdict.
e2e_github_worse_rc() {
    if [ "$1" -eq 0 ] || { [ "$1" -eq "$E2E_GITHUB_NO_VERDICT_EXIT" ] && [ "$2" -ne 0 ]; }; then
        echo "$2"
    else
        echo "$1"
    fi
}

# e2e_github_report <dir>: print every phase in e2e.sh order and return the
# first failing phase's exit code, or no verdict when no phase failed outright.
# Needs report_playwright_totals and report_project_exit_codes from
# scripts/lib/e2e.sh.
e2e_github_report() {
    local dir="$1" entries="$1/e2e-plan/entries"
    local overall=0 suite project id rc log phase_rc project_rcs=() combined

    if [ ! -s "$entries" ]; then
        echo "ERROR: the run uploaded no shard plan, so it produced no verdict." >&2
        return "$E2E_GITHUB_NO_VERDICT_EXIT"
    fi

    for suite in engine api wasm embedder; do
        while read -r id s _; do
            [ "$s" = "$suite" ] || continue
            echo ""
            echo "══ $suite (GitHub) ══"
            log="$(e2e_github_shard_log "$dir" "$id")"
            cat "$log" 2>/dev/null || echo "ERROR: no output from the $suite shard"
            rc="$(e2e_github_shard_rc "$dir" "$id")"
            echo "── $suite exit code: $rc ──"
            overall="$(e2e_github_worse_rc "$overall" "$rc")"
        done <"$entries"
    done

    for project in $(e2e_browser_projects); do
        grep -q " browser $project " "$entries" || continue
        echo ""
        echo "══ browser: $project (GitHub) ══"
        combined="$(mktemp "${TMPDIR:-/tmp}/lucidos-e2e-github-$project.XXXXXX")"
        phase_rc=0
        while read -r id s p _; do
            [ "$s" = browser ] && [ "$p" = "$project" ] || continue
            log="$(e2e_github_shard_log "$dir" "$id")"
            cat "$log" 2>/dev/null >>"$combined"
            rc="$(e2e_github_shard_rc "$dir" "$id")"
            phase_rc="$(e2e_github_worse_rc "$phase_rc" "$rc")"
        done <"$entries"
        cat "$combined"
        report_playwright_totals "$project" "$combined" || { [ "$phase_rc" -ne 0 ] || phase_rc=1; }
        rm -f "$combined"
        project_rcs+=("$project:$phase_rc")
        overall="$(e2e_github_worse_rc "$overall" "$phase_rc")"
    done

    if [ "${#project_rcs[@]}" -gt 0 ]; then
        report_project_exit_codes "$overall" "${project_rcs[@]}"
        overall=$?
    fi
    return "$overall"
}
