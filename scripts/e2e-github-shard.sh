#!/usr/bin/env bash
# e2e-github-shard.sh: the runner half of e2e GitHub mode (ADR 0382).
#
# Called only by .github/workflows/e2e.yml, on a fresh GitHub runner:
#
#   plan                       write the job matrices to $GITHUB_OUTPUT and
#                              the shard list to e2e-out/plan/entries
#   find-engine linux|macos    name an earlier run's engine built from this
#                              tree's Rust, and the newest target/ state, in
#                              $GITHUB_OUTPUT, or nothing; and add a line to
#                              e2e-out/plan/engines
#   build <out-dir> [<state-dir>]
#                              build the e2e engine, SDK and frontend into a
#                              tarball, plus the engine and target/ state
#                              later runs reuse. Starts from the target/ state
#                              in <state-dir>, when the plan found one
#   unpack <tarball>           put a build back where the harness looks for it
#   assemble <engine-dir>      reuse an earlier run's engine, then build the
#                              SDK and frontend; compile if none arrived
#   warm <suite> [<project>]   start the slow setup in the background: the
#                              Postgres image, system packages, npm deps and
#                              the one browser the project drives
#   cache-keys                 name the macOS caches seed-cache saves
#   seed-macos-cache <dir>     fill what those caches hold, the Postgres
#                              under <dir> as native-postgres puts it
#   native-postgres <dir>      start a Postgres with pgvector, for runners with no Docker
#   run <id> <suite> [<project> <shard> <total>]
#                              run one shard through the ordinary e2e scripts,
#                              or the engine suite through test-engine.sh. A
#                              failed engine test reruns once (e2e_flaky.sh)
#
# The request is .github/e2e-request.json, which scripts/e2e-github.sh wrote.

# No -u: the e2e libraries this sources are written for set -e callers only.
set -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
# shellcheck source=scripts/lib/e2e_flaky.sh
source "$SCRIPT_DIR/lib/e2e_flaky.sh"
# shellcheck source=scripts/lib/e2e_warm.sh
source "$SCRIPT_DIR/lib/e2e_warm.sh"
REQUEST="${E2E_GITHUB_REQUEST:-$PROJECT_DIR/.github/e2e-request.json}"
OUT_ROOT="${E2E_GITHUB_OUT:-$PROJECT_DIR/e2e-out}"

# Shards per browser project for a full run. 7 + 5 + 5 browser jobs plus the
# three other suites fill the public repo's 20 concurrent jobs, and macOS is
# capped at 5 there. Desktop Chromium runs every spec but `-mobile`, the phone
# projects only `-mobile` and `-all` (project_runs_spec), so it takes the extra
# shards.
declare_shards() {
    case "$1" in
        chromium) echo 7 ;;
        mobile | mobile-webkit) echo 5 ;;
    esac
}
SPECS_PER_SHARD_FLOOR=8
# The engine suite's run limit on a runner, compile included. The engine job's
# timeout-minutes in e2e.yml sits above it.
ENGINE_RUN_LIMIT_SECS=5400

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

# A runner has no logged-in Claude Code, only the fake, so a spec tagged
# @real-claude-code must never reach one. Needs e2e_spec_needs_real_claude_code.
refuse_real_claude_code_specs() {
    local spec refused=""
    while IFS= read -r spec; do
        [ -n "$spec" ] || continue
        if e2e_spec_needs_real_claude_code "$PROJECT_DIR/crates/lucidos-app/e2e/$spec"; then
            refused="$refused $spec"
        fi
    done <<EOF
$(request_field 'r["browser"]["specs"]')
EOF
    [ -z "$refused" ] || {
        echo "ERROR: these specs need the real Claude Code and run only on the local leg:$refused" >&2
        return 1
    }
}

# Where resolve_claude_binary (runtime/claude_code.rs) looks first, under the
# home directory. e2e_github_test.sh pins it to that function.
FAKE_CLAUDE_CODE_HOME_PATH=".local/bin/claude"
FAKE_CLAUDE_CODE="$PROJECT_DIR/crates/lucidos-app/e2e/fake-claude-code/claude.mjs"

# install_fake_claude_code [<home>]: link the fake Claude Code where the engine
# looks first. Only on a GitHub runner, and never over an existing claude.
install_fake_claude_code() {
    local target="${1:-$HOME}/$FAKE_CLAUDE_CODE_HOME_PATH"
    if [ "${GITHUB_ACTIONS:-}" != true ]; then
        echo "ERROR: the fake Claude Code installs only on a GitHub runner." >&2
        return 1
    fi
    if [ -e "$target" ] || [ -L "$target" ]; then
        echo "ERROR: $target already exists, and the fake never replaces a claude." >&2
        return 1
    fi
    mkdir -p "$(dirname "$target")" && ln -s "$FAKE_CLAUDE_CODE" "$target"
}

# ensure_main_branch [<repo>]: a coding-agent worktree branches from main, and
# the runner checks out only the e2e run branch. Name its HEAD main when no
# main exists. An existing main is never moved.
ensure_main_branch() {
    local repo="${1:-$PROJECT_DIR}"
    git -C "$repo" rev-parse --verify --quiet refs/heads/main >/dev/null \
        || git -C "$repo" branch main HEAD
}

cmd_plan() {
    # shellcheck source=scripts/lib/e2e.sh
    source "$SCRIPT_DIR/lib/e2e.sh"
    mkdir -p "$OUT_ROOT/plan"
    local entries="$OUT_ROOT/plan/entries" linux="" macos="" engine=false suite project count shards i id entry
    : >"$entries"
    while IFS= read -r suite; do
        case "$suite" in
            engine)
                # Its own job: it builds nothing the e2e shards share.
                echo "engine engine - 1 1" >>"$entries"
                engine=true
                ;;
            api | wasm | embedder)
                echo "$suite $suite - 1 1" >>"$entries"
                linux="$linux{\"id\":\"$suite\",\"suite\":\"$suite\",\"project\":\"-\",\"shard\":1,\"total\":1},"
                ;;
            browser)
                refuse_real_claude_code_specs || return 1
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
        echo "has_engine=$engine"
    } >>"${GITHUB_OUTPUT:-/dev/stdout}"
    cat "$entries"
}

# What the harness builds, and where it reads it back. One list, for both
# halves of the hand-over.
build_paths() {
    printf '%s\n' .launch packages/lucidos-sdk/dist crates/lucidos-app/dist
}

# Coverage requested: the build emits hidden sourcemaps, and the run is told to
# keep them. Both jobs export it, or the run's staleness check rebuilds without.
export_coverage_request() {
    [ "$(request_field 'r.get("coverage", False)')" = True ] || return 0
    export LUCIDOS_E2E_COVERAGE=1
}

# ── Engine reuse across runs (ADR 0536) ──
# A build uploads three artifacts, each named with the engine manifest
# pre-key and its run id:
#   e2e-manifest-<prekey>-<run>  the engine input manifest alone, small
#   e2e-engine-<prekey>-<run>    that manifest beside the four binaries
#   e2e-target-<prekey>-<run>    the target/ state the build left
# The plan job checks recent manifests against the tree and names the first
# match, so its test jobs skip the build. It also names the newest target/
# state, which a build job that runs restores first, so cargo compiles only
# what changed. Jobs download with actions/download-artifact, which is several
# times faster than the REST API on macOS runners.

# The cargo profile of every build here (Cargo.toml [profile.e2e]).
E2E_GITHUB_CARGO_PROFILE=e2e
# How many recent manifests a lookup reads before it gives up.
E2E_ENGINE_CANDIDATES=8
# How many pages of the artifact list a lookup reads, 100 artifacts each.
E2E_ARTIFACT_PAGES=5
# Where the state's own manifest travels inside target/.
TARGET_STATE_MANIFEST=lucidos-input-manifest

# The platform key each job's runner builds for (engine_manifest_platform).
github_platform() {
    case "$1" in
        linux) echo Linux-x86_64 ;;
        macos) echo Darwin-arm64 ;;
        *) echo "ERROR: unknown runner '$1'" >&2; return 1 ;;
    esac
}

use_e2e_profile() {
    export ENGINE_CARGO_PROFILE="$E2E_GITHUB_CARGO_PROFILE"
    export LUCIDOS_E2E_ENGINE_CACHE="${RUNNER_TEMP:-$OUT_ROOT}/e2e-engines"
}

# The seam the tests replace: GitHub's REST API, as the run's own token.
github_api() {
    gh api "$@"
}

# list_artifacts <name-prefix>: "<artifact id> <run id> <name>" per unexpired
# artifact, newest first.
list_artifacts() {
    local prefix="$1" page
    for ((page = 1; page <= E2E_ARTIFACT_PAGES; page++)); do
        github_api "repos/${GITHUB_REPOSITORY:?}/actions/artifacts?per_page=100&page=$page" \
            --jq '.artifacts[] | select(.expired | not) | "\(.id) \(.workflow_run.id) \(.name)"' \
            || return 1
    done | awk -v p="$prefix" 'index($3, p) == 1' | sort -rn -k1,1
}

# download_artifact <artifact id> <dir>: unzip one artifact into <dir>.
download_artifact() {
    local id="$1" dir="$2"
    mkdir -p "$dir" || return 1
    github_api "repos/${GITHUB_REPOSITORY:?}/actions/artifacts/$id/zip" >"$dir.zip" || return 1
    unzip -q -o "$dir.zip" -d "$dir" && rm -f "$dir.zip"
}

engine_prekey() { # [<platform>]
    engine_manifest_prekey "$PROJECT_DIR" "$E2E_GITHUB_CARGO_PROFILE" "$ENGINE_BUILD_FEATURES" "$@"
}

# Never fails the plan: any trouble reads as "no engine to reuse", and the
# build job runs.
cmd_find_engine() {
    local os="$1" platform prekey work id run_id name run="" artifact="" n=0
    # shellcheck source=scripts/lib/e2e.sh
    source "$SCRIPT_DIR/lib/e2e.sh"
    platform="$(github_platform "$os")" || return 1
    prekey="$(engine_prekey "$platform")"
    work="$(mktemp -d)"
    while read -r id run_id name; do
        [ -n "$id" ] || continue
        n=$((n + 1))
        [ "$n" -le "$E2E_ENGINE_CANDIDATES" ] || break
        if ! download_artifact "$id" "$work/$id"; then
            echo "Engine lookup: could not download $name"
            continue
        fi
        if engine_manifest_check "$PROJECT_DIR" "$work/$id/manifest" \
            "$E2E_GITHUB_CARGO_PROFILE" "$ENGINE_BUILD_FEATURES" "$platform" 2>"$work/reason"; then
            run="$run_id"
            artifact="e2e-engine-${name#e2e-manifest-}"
            break
        fi
        echo "Engine lookup: $name does not match: $(cat "$work/reason")"
    done <<EOF
$(list_artifacts "e2e-manifest-$prekey-" || echo "Engine lookup: could not list artifacts" >&2)
EOF
    rm -rf "$work"
    read -r id run_id name <<EOF
$(list_artifacts "e2e-target-$prekey-" 2>/dev/null | head -n 1)
EOF
    if [ -n "$run" ]; then
        echo "Engine lookup ($os): REUSING $artifact from run $run"
    else
        echo "Engine lookup ($os): no engine built from this tree's Rust, so the build job runs"
    fi
    # The driver's timing record reads which engine each platform reused.
    mkdir -p "$OUT_ROOT/plan"
    echo "$os ${run:--}" >>"$OUT_ROOT/plan/engines"
    {
        echo "${os}_engine_run=$run"
        echo "${os}_engine_artifact=$artifact"
        echo "${os}_target_run=${run_id:-}"
        echo "${os}_target_artifact=${name:-}"
    } >>"${GITHUB_OUTPUT:-/dev/stdout}"
}

# restore_target_state <state-dir>: unpack the target/ state the plan found,
# downloaded into <state-dir>, then age every unchanged input so cargo redoes
# only what changed. Never fails the build: without state it compiles from
# scratch.
restore_target_state() {
    local state="$1/e2e-target.tar.zst" manifest="$PROJECT_DIR/target/$E2E_GITHUB_CARGO_PROFILE/$TARGET_STATE_MANIFEST"
    if [ ! -f "$state" ]; then
        echo "Target state: none arrived, compiling from scratch"
        return 0
    fi
    if zstd -dc "$state" | tar -C "$PROJECT_DIR" -xf - && [ -f "$manifest" ]; then
        echo "Target state: restored"
        engine_manifest_stamp_mtimes "$PROJECT_DIR" "$manifest"
    else
        echo "Target state: could not restore it, compiling from scratch"
    fi
}

prepare_build() {
    export_coverage_request
    # lib/e2e.sh halves the cores to spare a shared Mac. A runner is a VM with
    # nothing else on it.
    export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc 2>/dev/null || sysctl -n hw.ncpu)}"
    use_e2e_profile
    # Swatinem/rust-cache exports CARGO_INCREMENTAL=0, which overrides the e2e
    # profile's incremental = true and makes every change recompile the engine
    # whole.
    unset CARGO_INCREMENTAL
    # shellcheck source=scripts/lib/e2e.sh
    source "$SCRIPT_DIR/lib/e2e.sh"
    e2e_workspace_env
}

cmd_build() {
    local out="$1" state="${2:-}" prekey engine profile_dir
    prepare_build
    mkdir -p "$out"
    prekey="$(engine_prekey)"
    profile_dir="$PROJECT_DIR/target/$E2E_GITHUB_CARGO_PROFILE"
    [ -z "$state" ] || restore_target_state "$state"
    mkdir -p "$out/diag"
    { echo "cpus=$(nproc 2>/dev/null || sysctl -n hw.ncpu) jobs=$CARGO_BUILD_JOBS"; rustc -vV; env | grep -E '^(CARGO|RUST)' ; } | tee "$out/diag/env.txt"
    ls -la "$profile_dir/incremental" 2>&1 | tee "$out/diag/incr-before.txt"
    du -sh "$profile_dir"/incremental/* 2>&1 | tee -a "$out/diag/incr-before.txt"
    export CARGO_LOG=cargo::core::compiler::fingerprint=info RUSTC_LOG=rustc_incremental=info
    local t0=$SECONDS
    build_e2e_engine_once 2>"$out/diag/build-stderr.log" || { tail -100 "$out/diag/build-stderr.log"; return 1; }
    echo "build_e2e_engine_once took $((SECONDS - t0))s"
    unset CARGO_LOG RUSTC_LOG
    grep -E "Compiling|Finished|dirty|stale" "$out/diag/build-stderr.log" | head -80
    ls -la "$profile_dir/incremental" 2>&1 | tee "$out/diag/incr-after.txt"
    cp -R "$PROJECT_DIR/target/cargo-timings" "$out/diag/" 2>/dev/null || true
    ensure_frontend_built || return 1
    # shellcheck disable=SC2046 # build_paths prints one plain path per line
    tar -C "$PROJECT_DIR" -czf "$out/e2e-build.tar.gz" $(build_paths)
    echo "prekey=$prekey" >>"${GITHUB_OUTPUT:-/dev/null}"
    engine="$(engine_cache_find "$PROJECT_DIR" "$E2E_GITHUB_CARGO_PROFILE" "$ENGINE_BUILD_FEATURES")"
    if [ -z "$engine" ]; then
        echo "WARNING: this build left no engine manifest, so no later run can reuse it."
        return 0
    fi
    cp "$engine/manifest" "$out/manifest"
    tar -C "$(dirname "$engine")" -cf "$out/e2e-engine.tar" "$(basename "$engine")"
    # The whole profile directory, dependencies included: the dependency cache
    # holds no e2e-profile build, and a state that left out what it restored
    # made the next build compile everything.
    cp "$engine/manifest" "$profile_dir/$TARGET_STATE_MANIFEST"
    tar -C "$PROJECT_DIR" -cf - "target/$E2E_GITHUB_CARGO_PROFILE" | zstd -T0 -3 -q -o "$out/e2e-target.tar.zst" \
        || { echo "WARNING: could not pack the target/ state."; rm -f "$out/e2e-target.tar.zst"; }
}

# The test-job half of a reuse: the engine from an earlier run, then the
# generated files, SDK bundles and frontend this tree needs. Without an engine
# (an expired artifact, a failed download) it compiles one, slowly but right.
cmd_assemble() {
    local in="$1"
    e2e_warm_wait npm || return 1
    prepare_build
    if [ -f "$in/e2e-engine.tar" ]; then
        mkdir -p "$LUCIDOS_E2E_ENGINE_CACHE"
        tar -C "$LUCIDOS_E2E_ENGINE_CACHE" -xf "$in/e2e-engine.tar"
    else
        echo "WARNING: no reusable engine arrived, so this job compiles one."
    fi
    build_e2e_engine_once || return 1
    ensure_frontend_built
}

# -m stamps the files now. They are then newer than the fresh checkout, so the
# harness's staleness check reuses them instead of rebuilding.
cmd_unpack() {
    tar -C "$PROJECT_DIR" -xmzf "$1"
}

# ── Warm-up (scripts/lib/e2e_warm.sh) ──
# Nothing here needs the engine, so a test job starts it right after checkout.
# Each later step waits only for its own part: assemble and run for npm, and
# e2e-browser.sh for the rest, just before Playwright starts.

# The Playwright the lockfile pins, readable before npm ci finishes.
locked_playwright_version() {
    node -p "require('$PROJECT_DIR/package-lock.json').packages['node_modules/playwright'].version"
}

warm_npm() {
    ensure_npm_deps "$PROJECT_DIR" "workspace dependencies"
}

# The runner image preinstalls the Microsoft apt source, and nothing here
# installs from it. When packages.microsoft.com answers 403, every apt-get
# update fails, install-deps included, and the whole shard with it.
drop_microsoft_apt_source() {
    sudo rm -f "${APT_SOURCES_DIR:-/etc/apt/sources.list.d}"/microsoft*
}

# A stalled mirror made apt retry without a bound and hold the warm-up for
# 900s. Every apt-get in the chain, install-deps' included, reads this file.
bound_apt_network() {
    printf 'Acquire::http::Timeout "20";\nAcquire::https::Timeout "20";\nAcquire::Retries "2";\nDPkg::Lock::Timeout "60";\n' \
        | sudo tee "${APT_CONF_DIR:-/etc/apt/apt.conf.d}/99-e2e-bounded-network" >/dev/null
}

# run_with_deadline <seconds> <cmd...>: the command runs as its own process
# group, and the whole group gets SIGTERM at the deadline. Returns 124 then.
run_with_deadline() {
    local seconds="$1" pid watchdog status
    shift
    set -m
    "$@" &
    pid=$!
    set +m
    (sleep "$seconds"; kill -TERM -- "-$pid") >/dev/null 2>&1 &
    watchdog=$!
    wait "$pid" && status=0 || status=$?
    if kill "$watchdog" 2>/dev/null; then
        wait "$watchdog" 2>/dev/null || true
    else
        status=124
    fi
    return "$status"
}

# One apt chain, so dpkg never runs twice at once. install-deps refreshes the
# package lists, and the second install reuses them.
apt_chain() {
    if [ "$1" = browser ]; then
        npx --yes "playwright@$(locked_playwright_version)" install-deps chromium || return 1
    else
        sudo apt-get update -qq || return 1
    fi
    sudo apt-get install -y -qq ripgrep postgresql-client
}

# sudo runs apt-get in its own session, so the deadline's group kill can miss
# it. The survivor then holds the dpkg lock and fails every later attempt.
# The runner is single-use, so matching the exact name is safe.
reap_stalled_apt() {
    sudo pkill -TERM -x apt-get || true
}

# A slow mirror that keeps trickling bytes defeats every apt timeout, so each
# attempt also gets a deadline, and a stalled one is retried.
warm_apt() {
    drop_microsoft_apt_source
    bound_apt_network
    local attempt
    for attempt in 1 2 3; do
        run_with_deadline "${APT_ATTEMPT_SECONDS:-240}" apt_chain "$1" && return 0
        echo "apt warm-up attempt $attempt failed" >&2
        reap_stalled_apt
    done
    return 1
}

# warm_browsers <project>: only the browser that project drives. A shard is
# always headless, and headless Chromium runs the headless shell.
warm_browsers() {
    local -a browsers=(--only-shell chromium)
    [ "$1" != mobile-webkit ] || browsers=(webkit)
    e2e_warm_wait npm || return 1
    (cd "$PROJECT_DIR/crates/lucidos-app" && npx playwright install "${browsers[@]}")
}

cmd_warm() {
    local suite="$1" project="${2:-}"
    # shellcheck source=scripts/lib/e2e.sh
    source "$SCRIPT_DIR/lib/e2e.sh"
    export LUCIDOS_E2E_WARM_DIR="${RUNNER_TEMP:-$OUT_ROOT}/e2e-warm"
    echo "LUCIDOS_E2E_WARM_DIR=$LUCIDOS_E2E_WARM_DIR" >>"${GITHUB_ENV:-/dev/null}"
    # A macOS runner has no Docker, and native-postgres starts its own.
    if [ "$(uname -s)" = Linux ]; then
        e2e_warm_start pg-image docker pull -q "$PG_IMAGE"
        e2e_warm_start apt warm_apt "$suite"
    fi
    e2e_warm_start npm warm_npm
    e2e_warm_start gcloud warm_gcloud
    [ "$suite" != browser ] || e2e_warm_start browsers warm_browsers "$project"
}

# The engine asks gcloud for a Vertex project at boot. A runner's first gcloud
# call took 5 to 20 seconds and a second one half a second, so this makes the
# first call here. The answer itself does not matter.
warm_gcloud() {
    command -v gcloud >/dev/null || return 0
    gcloud config get-value project >/dev/null 2>&1 || true
}

# The versions the release bundles, read from the one place that pins them.
pinned_version() {
    sed -n "s/^$1=\"\${$1:-\([0-9.]*\)}\".*/\1/p" "$SCRIPT_DIR/build-headless.sh"
}

pinned_postgres_versions() {
    PG_VERSION="$(pinned_version PG_VERSION)"
    PGVECTOR_VERSION="$(pinned_version PGVECTOR_VERSION)"
    [ -n "$PG_VERSION" ] && [ -n "$PGVECTOR_VERSION" ] \
        || { echo "ERROR: could not read the pinned Postgres versions from build-headless.sh" >&2; return 1; }
}

# The macOS Postgres native-postgres starts, under its work directory.
MACOS_PG_TRIPLE=aarch64-apple-darwin

# The two caches seed-cache saves on main and the macOS job restores: the
# Postgres native-postgres compiles pgvector into, and the WebKit build.
cmd_cache_keys() {
    pinned_postgres_versions || return 1
    {
        echo "postgres_dir=postgresql-$PG_VERSION-$MACOS_PG_TRIPLE"
        echo "postgres=postgresql-$PG_VERSION-$MACOS_PG_TRIPLE-pgvector-$PGVECTOR_VERSION"
        echo "playwright=$(locked_playwright_version)"
    } >>"${GITHUB_OUTPUT:-/dev/stdout}"
}

cmd_seed_macos_cache() {
    local work="$1"
    pinned_postgres_versions || return 1
    # shellcheck source=scripts/lib/stage_runtime.sh
    source "$SCRIPT_DIR/lib/stage_runtime.sh"
    # shellcheck source=scripts/lib/e2e.sh
    source "$SCRIPT_DIR/lib/e2e.sh"
    stage_runtime_fetch_postgres "$PG_VERSION" "$PGVECTOR_VERSION" "$MACOS_PG_TRIPLE" "$work" >/dev/null || return 1
    warm_npm || return 1
    (cd "$PROJECT_DIR/crates/lucidos-app" && npx playwright install webkit)
}

cmd_native_postgres() {
    local work="$1" prefix data port=5432
    pinned_postgres_versions || return 1
    # shellcheck source=scripts/lib/stage_runtime.sh
    source "$SCRIPT_DIR/lib/stage_runtime.sh"
    # shellcheck source=scripts/lib/workspace_constants.sh
    source "$SCRIPT_DIR/lib/workspace_constants.sh"
    # shellcheck source=scripts/lib/pg_secret.sh
    source "$SCRIPT_DIR/lib/pg_secret.sh"
    # A restored cache holds the compiled prefix, so this only starts it.
    prefix="$(stage_runtime_fetch_postgres "$PG_VERSION" "$PGVECTOR_VERSION" "$MACOS_PG_TRIPLE" "$work")" || return 1
    data="$work/data"
    ( umask 077; shared_pg_password >"$work/pwfile" ) || return 1
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
    # e2e-browser.sh waits for the system packages and the browser itself,
    # after the engine has booted. gcloud's first call must end before it.
    if [ "$suite" = browser ]; then
        e2e_warm_wait npm gcloud || return 1
    else
        e2e_warm_wait || return 1
    fi
    # Every suite must see what the build job saw: dist/ holds sourcemaps exactly
    # when coverage was requested, and a mismatch makes the suite rebuild dist/
    # on a runner that never installed node_modules.
    export_coverage_request

    # The binaries came from the build job. The runner is a disposable VM, not
    # this Mac: the absolute available floor gives way to the share of its RAM,
    # and the load guard, which protects a shared host, stands down. A macOS
    # runner idles at a load above that guard's cap.
    export LUCIDOS_E2E_ENGINE_BUILT=1
    use_e2e_profile
    export LUCIDOS_E2E_FREE_FLOOR_MIN_GB=1
    export HOST_LOAD_GUARD_DISABLE=1
    # A shard IS the GitHub run, so its e2e scripts must never hand off again.
    export LUCIDOS_E2E_LOCAL=1

    local cmd=()
    case "$suite" in
        engine)
            # A cold runner compiles the whole test binary inside the run limit.
            export LUCIDOS_TEST_RUN_LIMIT_SECS="${LUCIDOS_TEST_RUN_LIMIT_SECS:-$ENGINE_RUN_LIMIT_SECS}"
            cmd=("$SCRIPT_DIR/test-engine.sh")
            ;;
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
            # A real gateway lists and adopts the engine, as on an install, so
            # the gateway-dependent specs test something real (ADR 0385).
            export LUCIDOS_E2E_GATEWAY=own
            # The fixture records only on Chromium, into the uploaded output.
            [ -z "${LUCIDOS_E2E_COVERAGE:-}" ] || export LUCIDOS_E2E_COVERAGE_DIR="$out/coverage"
            install_fake_claude_code || return 1
            ensure_main_branch || return 1
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
    if [ "$suite" = engine ] && [ "$rc" -ne 0 ]; then
        e2e_rerun_failed_engine_tests "$out/$id.log" "${cmd[0]}" && rc=0
    fi
    echo "$rc" >"$out/$id.rc"
    return "$rc"
}

# Sourced by scripts/lib/e2e_github_test.sh, which calls the functions alone.
[ "${BASH_SOURCE[0]}" = "$0" ] || return 0

case "${1:-}" in
    plan) cmd_plan ;;
    find-engine) cmd_find_engine "${2:?linux or macos}" ;;
    build) cmd_build "${2:?out dir}" "${3:-}" ;;
    unpack) cmd_unpack "${2:?tarball}" ;;
    assemble) cmd_assemble "${2:?engine dir}" ;;
    warm) cmd_warm "${2:?suite}" "${3:-}" ;;
    cache-keys) cmd_cache_keys ;;
    seed-macos-cache) cmd_seed_macos_cache "${2:?work dir}" ;;
    native-postgres) cmd_native_postgres "${2:?work dir}" ;;
    run) shift; cmd_run "$@" ;;
    *) echo "usage: $0 plan|find-engine|build|unpack|assemble|warm|cache-keys|seed-macos-cache|native-postgres|run" >&2; exit 1 ;;
esac
