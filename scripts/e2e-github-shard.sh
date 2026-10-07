#!/usr/bin/env bash
# e2e-github-shard.sh: the runner half of e2e GitHub mode (ADR 0382).
#
# Called only by .github/workflows/e2e.yml, on a fresh GitHub runner:
#
#   plan                       write the job matrices to $GITHUB_OUTPUT and
#                              the shard list to e2e-out/plan/entries
#   build <out-dir>            build the e2e engine, SDK and frontend into a tarball
#   unpack <tarball>           put a build back where the harness looks for it
#   native-postgres <dir>      start a Postgres with pgvector, for runners with no Docker
#   run <id> <suite> [<project> <shard> <total>]
#                              run one shard through the ordinary e2e scripts
#
# The request is .github/e2e-request.json, which scripts/e2e-github.sh wrote.

# No -u: the e2e libraries this sources are written for set -e callers only.
set -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
REQUEST="${E2E_GITHUB_REQUEST:-$PROJECT_DIR/.github/e2e-request.json}"
OUT_ROOT="${E2E_GITHUB_OUT:-$PROJECT_DIR/e2e-out}"

# Shards per browser project for a full run. 6 + 6 + 5 browser jobs plus the
# three other suites fill the public repo's 20 concurrent jobs, and macOS is
# capped at 5 there.
declare_shards() {
    case "$1" in
        chromium | mobile) echo 6 ;;
        mobile-webkit) echo 5 ;;
    esac
}
SPECS_PER_SHARD_FLOOR=8

# request_field <python expression over `r`>: one value per line.
request_field() {
    python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
v = eval(sys.argv[2])
for x in (v if isinstance(v, list) else [v]):
    print(x)
' "$REQUEST" "$1"
}

# The specs a project runs, from the request's list. Needs project_runs_spec.
project_specs() {
    local project="$1" spec
    while IFS= read -r spec; do
        [ -n "$spec" ] || continue
        project_runs_spec "$project" "$spec" && printf '%s\n' "$spec"
    done <<EOF
$(request_field 'r["browser"]["specs"]')
EOF
}

cmd_plan() {
    # shellcheck source=scripts/lib/e2e.sh
    source "$SCRIPT_DIR/lib/e2e.sh"
    mkdir -p "$OUT_ROOT/plan"
    local entries="$OUT_ROOT/plan/entries" linux="" macos="" suite project count shards i id entry
    : >"$entries"
    while IFS= read -r suite; do
        case "$suite" in
            api | wasm | embedder)
                echo "$suite $suite - 1 1" >>"$entries"
                linux="$linux{\"id\":\"$suite\",\"suite\":\"$suite\",\"project\":\"-\",\"shard\":1,\"total\":1},"
                ;;
            browser)
                while IFS= read -r project; do
                    [ -n "$project" ] || continue
                    count="$(project_specs "$project" | grep -c .)"
                    [ "$count" -gt 0 ] || continue
                    shards=$(( (count + SPECS_PER_SHARD_FLOOR - 1) / SPECS_PER_SHARD_FLOOR ))
                    [ "$shards" -le "$(declare_shards "$project")" ] || shards="$(declare_shards "$project")"
                    for ((i = 1; i <= shards; i++)); do
                        id="$project-$i-$shards"
                        echo "$id browser $project $i $shards" >>"$entries"
                        entry="{\"id\":\"$id\",\"suite\":\"browser\",\"project\":\"$project\",\"shard\":$i,\"total\":$shards},"
                        if [ "$project" = mobile-webkit ]; then macos="$macos$entry"; else linux="$linux$entry"; fi
                    done
                done <<EOF
$(request_field 'r["browser"]["projects"]')
EOF
                ;;
        esac
    done <<EOF
$(request_field 'r["suites"]')
EOF
    {
        echo "linux={\"include\":[${linux%,}]}"
        echo "macos={\"include\":[${macos%,}]}"
        echo "has_linux=$([ -n "$linux" ] && echo true || echo false)"
        echo "has_macos=$([ -n "$macos" ] && echo true || echo false)"
    } >>"${GITHUB_OUTPUT:-/dev/stdout}"
    cat "$entries"
}

# What the harness builds, and where it reads it back. One list, for both
# halves of the hand-over.
build_paths() {
    printf '%s\n' .launch packages/lucidos-sdk/dist crates/lucidos-app/dist
}

cmd_build() {
    local out="$1"
    # lib/e2e.sh halves the cores to spare a shared Mac. A runner is a VM with
    # nothing else on it.
    export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc 2>/dev/null || sysctl -n hw.ncpu)}"
    # shellcheck source=scripts/lib/e2e.sh
    source "$SCRIPT_DIR/lib/e2e.sh"
    e2e_workspace_env
    build_e2e_engine_once || return 1
    ensure_frontend_built || return 1
    mkdir -p "$out"
    # shellcheck disable=SC2046 # build_paths prints one plain path per line
    tar -C "$PROJECT_DIR" -czf "$out/e2e-build.tar.gz" $(build_paths)
}

# -m stamps the files now. They are then newer than the fresh checkout, so the
# harness's staleness check reuses them instead of rebuilding.
cmd_unpack() {
    tar -C "$PROJECT_DIR" -xmzf "$1"
}

# The versions the release bundles, read from the one place that pins them.
pinned_version() {
    sed -n "s/^$1=\"\${$1:-\([0-9.]*\)}\".*/\1/p" "$SCRIPT_DIR/build-headless.sh"
}

cmd_native_postgres() {
    local work="$1" pg_version pgvector_version prefix data port=5432
    pg_version="$(pinned_version PG_VERSION)"
    pgvector_version="$(pinned_version PGVECTOR_VERSION)"
    [ -n "$pg_version" ] && [ -n "$pgvector_version" ] \
        || { echo "ERROR: could not read the pinned Postgres versions from build-headless.sh" >&2; return 1; }
    # shellcheck source=scripts/lib/stage_runtime.sh
    source "$SCRIPT_DIR/lib/stage_runtime.sh"
    # shellcheck source=scripts/lib/workspace_constants.sh
    source "$SCRIPT_DIR/lib/workspace_constants.sh"
    prefix="$(stage_runtime_fetch_postgres "$pg_version" "$pgvector_version" aarch64-apple-darwin "$work")" || return 1
    data="$work/data"
    printf '%s' "$PG_PASSWORD" >"$work/pwfile"
    "$prefix/bin/initdb" -D "$data" -U "$PG_USER" --pwfile="$work/pwfile" -A scram-sha-256 >/dev/null || return 1
    "$prefix/bin/pg_ctl" -D "$data" -l "$work/postgres.log" -w \
        -o "-p $port -c listen_addresses=127.0.0.1 -c max_connections=$PG_MAX_CONNECTIONS" start >/dev/null || return 1
    echo "$prefix/bin" >>"${GITHUB_PATH:-/dev/null}"
    echo "LUCIDOS_EXTERNAL_PG_PORT=$port" >>"${GITHUB_ENV:-/dev/stdout}"
}

cmd_run() {
    local id="$1" suite="$2" project="${3:-}" shard="${4:-}" total="${5:-}"
    local out="$OUT_ROOT/$id" rc
    mkdir -p "$out"

    # The binaries came from the build job. The runner is a disposable VM, not
    # this Mac: the absolute available floor gives way to the share of its RAM,
    # and the load guard, which protects a shared host, stands down. A macOS
    # runner idles at a load above that guard's cap.
    export LUCIDOS_E2E_ENGINE_BUILT=1
    export LUCIDOS_E2E_FREE_FLOOR_MIN_GB=1
    export HOST_LOAD_GUARD_DISABLE=1

    local cmd=()
    case "$suite" in
        api)
            cmd=("$SCRIPT_DIR/e2e-api.sh")
            while IFS= read -r a; do [ -z "$a" ] || cmd+=("$a"); done <<EOF
$(request_field 'r["api_args"]')
EOF
            ;;
        wasm) cmd=("$SCRIPT_DIR/e2e-wasm.sh") ;;
        embedder) cmd=("$SCRIPT_DIR/e2e-embedder.sh") ;;
        browser)
            # shellcheck source=scripts/lib/e2e.sh
            source "$SCRIPT_DIR/lib/e2e.sh"
            cmd=("$SCRIPT_DIR/e2e-browser.sh")
            [ "$project" = mobile-webkit ] || cmd+=(--no-webkit)
            cmd+=(-- "--project=$project" "--shard=$shard/$total")
            local spec a
            while IFS= read -r spec; do
                [ -z "$spec" ] || cmd+=("$(playwright_file_filter "$spec")")
            done <<EOF
$(project_specs "$project")
EOF
            while IFS= read -r a; do [ -z "$a" ] || cmd+=("$a"); done <<EOF
$(request_field 'r["browser"]["pw_args"]')
EOF
            ;;
        *) echo "ERROR: unknown suite '$suite'" >&2; return 1 ;;
    esac

    "${cmd[@]}" 2>&1 | tee "$out/$id.log"
    rc="${PIPESTATUS[0]}"
    echo "$rc" >"$out/$id.rc"
    return "$rc"
}

# Sourced by scripts/lib/e2e_github_test.sh, which calls the functions alone.
[ "${BASH_SOURCE[0]}" = "$0" ] || return 0

case "${1:-}" in
    plan) cmd_plan ;;
    build) cmd_build "${2:?out dir}" ;;
    unpack) cmd_unpack "${2:?tarball}" ;;
    native-postgres) cmd_native_postgres "${2:?work dir}" ;;
    run) shift; cmd_run "$@" ;;
    *) echo "usage: $0 plan|build|unpack|native-postgres|run" >&2; exit 1 ;;
esac
