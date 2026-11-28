#!/usr/bin/env bash

set -eo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/main_check.sh
source "$script_dir/../lib/main_check.sh"
# shellcheck source=scripts/lib/e2e.sh
source "$script_dir/../lib/e2e.sh"

workflow_path=".github/workflows/$E2E_GITHUB_WORKFLOW"
if [[ "$1" == --workflow ]]; then
    jq -cn --arg repo "$E2E_GITHUB_REPO" --arg path "$workflow_path" \
        --arg branch_prefix "$E2E_GITHUB_BRANCH_PREFIX" \
        --arg run_base "$(e2e_github_run_url '')" \
        '{repo: $repo, path: $path, branch_prefix: $branch_prefix, run_base: $run_base}'
    exit 0
fi

run_url="$1"
run_id="${run_url##*/}"
branch="$2"
output="$3"
if [[ ! "$run_id" =~ ^[0-9]+$ || "$branch" != "$E2E_GITHUB_BRANCH_PREFIX"* || "$run_url" != "$(e2e_github_run_url "$run_id")" ]]; then
    echo "Main check shadow received an invalid run link." >&2
    exit 1
fi
: "${GH_ACCOUNT_GITHUB_COM:?Set GH_ACCOUNT_GITHUB_COM for the shadow reader}"

endpoint="repos/$E2E_GITHUB_REPO/actions/runs/$run_id"
run="$(e2e_github_gh api "$endpoint")"
if ! jq -e --arg branch "$branch" --arg repo "$E2E_GITHUB_REPO" --arg path "$workflow_path" '
    .head_branch == $branch and .repository.full_name == $repo and
    .head_repository.full_name == $repo and .event == "push" and
    .path == $path
' <<<"$run" >/dev/null; then
    echo "Main check shadow could not verify the workflow run identity." >&2
    exit 1
fi

status="$(jq -er '.status | strings | select(length > 0)' <<<"$run")"
if [[ "$status" != completed ]]; then
    printf '%s\n' 'main-check pending'
    exit 0
fi

conclusion="$(jq -r '.conclusion' <<<"$run")"
case "$conclusion" in
    success|failure|cancelled|timed_out|action_required|neutral|skipped|stale|startup_failure) ;;
    *)
        echo "Main check shadow saw an unrecognized workflow conclusion." >&2
        exit 1
        ;;
esac

report="$output/report.log"
rc=0
artifacts_unavailable=false
if e2e_github_download "$run_id" "$output"; then
    e2e_github_report "$output" >"$report" 2>&1 || rc=$?
else
    available="$(e2e_github_gh api "$endpoint/artifacts" --paginate --jq '
        .artifacts[] | select(.expired == false) |
        select(.name == "e2e-plan" or (.name | startswith("e2e-out-"))) | .id
    ')"
    [[ -z "$available" ]] || exit 1
    artifacts_unavailable=true
fi

latest="$(e2e_github_gh api "$endpoint" --jq '[.status, .run_attempt] | @tsv')"
if [[ "$latest" != "$(jq -r '[.status, .run_attempt] | @tsv' <<<"$run")" ]]; then
    echo "Main check shadow saw a new attempt while reading the report." >&2
    exit 1
fi

if "$artifacts_unavailable"; then
    printf '%s%s\n' "$MAIN_CHECK_VERDICT_PREFIX" \
        '{"verdict": "unavailable", "summary": "No shard artifacts remain for the run."}'
else
    main_check_verdict_line "$(main_check_verdict "$rc")" "$rc" "$report"
fi
