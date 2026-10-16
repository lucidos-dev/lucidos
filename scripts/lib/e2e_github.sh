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

E2E_GITHUB_REPO="lucidos-dev/lucidos"
E2E_GITHUB_REMOTE="lucidos"
E2E_GITHUB_WORKFLOW="e2e.yml"
E2E_GITHUB_BRANCH_PREFIX="e2e/"
E2E_GITHUB_REQUEST_PATH=".github/e2e-request.json"
E2E_GITHUB_AUTHOR_NAME="Lucidos e2e"
E2E_GITHUB_AUTHOR_EMAIL="lucidos-e2e@users.noreply.github.com"
E2E_GITHUB_SWEEP_AGE_SECS=86400
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

# e2e_github_handoff <suite> "$@": GitHub is the default. Replace this process
# with the driver unless the run must stay here: --local, a local-only flag or
# variable, an inherited LUCIDOS_E2E_LOCAL, or an unmet precondition. Only flags before
# any `--` count. Staying here exports LUCIDOS_E2E_LOCAL=1, so every nested e2e
# script stays too. Each e2e script calls this first, before the e2e lock, and
# then parses E2E_ARGS: its arguments minus --local and --github.
e2e_github_handoff() {
    local suite="$1" arg github="" local_run="" past_dashes="" reason="" env_name
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
    elif [ -z "$local_run" ] && [ -z "$github" ]; then
        reason="$(e2e_github_unavailable_reason "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)")"
        if [ -n "$reason" ]; then
            echo "[e2e] running on this host: $reason." >&2
            local_run=1
        fi
    fi
    if [ -n "$local_run" ]; then
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

# e2e_github_e2e_lock_holder: name the live holder of this host's e2e lock, or
# print nothing when it is free or its holder is dead. Needs e2e_lock.sh.
e2e_github_e2e_lock_holder() {
    _e2e_read_lock_file "$(_e2e_lock_path)" || return 0
    [ -n "$_E2E_LK_PID" ] && kill -0 "$_E2E_LK_PID" 2>/dev/null || return 0
    if [ -n "$_E2E_LK_THREAD" ]; then
        printf 'thread %s\n' "$_E2E_LK_THREAD"
    else
        printf 'pid %s\n' "$_E2E_LK_PID"
    fi
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

# e2e_github_wait <run-id>: block until the run completes, printing progress
# whenever it changes. Prints the conclusion on the last line of stdout.
# Gives up after E2E_GITHUB_RUN_WAIT_SECS (default 6 hours) with the no-verdict
# exit. The default sits above the longest chain of timeout-minutes in e2e.yml,
# with room for queueing. A gh that keeps failing must still end the wait.
e2e_github_wait() {
    local run_id="$1" poll="${E2E_GITHUB_POLL_SECS:-30}" limit="${E2E_GITHUB_RUN_WAIT_SECS:-21600}"
    local json status line last="" deadline
    deadline=$(($(e2e_github_now) + limit))
    echo "[e2e-github] watching https://github.com/$E2E_GITHUB_REPO/actions/runs/$run_id" >&2
    while :; do
        if json="$(gh run view "$run_id" --repo "$E2E_GITHUB_REPO" --json status,conclusion,jobs 2>/dev/null)"; then
            line="$(e2e_github_run_progress "$json")"
            [ "$line" = "$last" ] || { echo "[e2e-github] $line" >&2; last="$line"; }
            status="$(printf '%s' "$json" | python3 -c 'import json, sys; print(json.load(sys.stdin)["status"])')"
            if [ "$status" = "completed" ]; then
                printf '%s' "$json" | python3 -c 'import json, sys; print(json.load(sys.stdin)["conclusion"] or "")'
                return 0
            fi
        fi
        if [ "$(e2e_github_now)" -ge "$deadline" ]; then
            echo "ERROR: run $run_id did not complete within ${limit}s, so it produced no verdict." >&2
            return "$E2E_GITHUB_NO_VERDICT_EXIT"
        fi
        sleep "$poll"
    done
}

e2e_github_cancel() {
    gh run cancel "$1" --repo "$E2E_GITHUB_REPO" >/dev/null 2>&1 || true
}

# e2e_github_stop_run <repo> <branch> <unfinished-run-id> <pushed>: on the way
# out, cancel a run that has not finished and delete the branch once pushed.
# It often runs after a SIGTERM that took the reader of stderr too, so it
# ignores SIGPIPE: a broken pipe must not stop it before either call.
e2e_github_stop_run() {
    local repo="$1" branch="$2" run_db_id="$3" pushed="$4"
    trap '' PIPE
    if [ -n "$run_db_id" ]; then
        echo "[e2e-github] cancelling run $run_db_id" >&2
        e2e_github_cancel "$run_db_id"
    fi
    if [ -n "$pushed" ]; then
        e2e_github_delete_branch "$repo" "$branch" || echo "[e2e-github] could not delete $branch; the next run's sweep will" >&2
    fi
}

e2e_github_download() {
    local run_id="$1" dir="$2"
    mkdir -p "$dir"
    gh run download "$run_id" --repo "$E2E_GITHUB_REPO" -D "$dir" -p 'e2e-out-*' -p 'e2e-plan'
}

# ── The report ───────────────────────────────────────────────────────────
#
# The plan artifact lists one line per shard: "<id> <suite> <project> <shard>
# <total>". Each shard uploads e2e-out-<id>/<id>.log and <id>.rc. A shard the
# plan names but whose output is missing has no verdict. So does a shard whose
# build the memory gate refused, since it tested nothing.

# e2e_github_shard_rc <dir> <id>: the shard's exit code, or the no-verdict code.
# Needs HOST_MEMORY_BUILD_REFUSED_EXIT, which scripts/lib/e2e.sh sources.
e2e_github_shard_rc() {
    local rc_file="$1/e2e-out-$2/$2.rc" rc
    rc="$(cat "$rc_file" 2>/dev/null)"
    case "$rc" in
        '' | *[!0-9]*) echo "$E2E_GITHUB_NO_VERDICT_EXIT" ;;
        "$HOST_MEMORY_BUILD_REFUSED_EXIT")
            echo "ERROR: the $2 shard's build never started (exit $rc), so it tested nothing." >&2
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
