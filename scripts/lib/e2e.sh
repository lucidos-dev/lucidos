#!/bin/bash

# Honor an inherited value — a hard assignment here would clobber the sandbox a
# test pins before sourcing us, and this variable is load-bearing for signalling:
# `e2e_workspace_env` exports it as $WORKSPACE, which resolves $ENGINE_PIDFILE,
# which `stop_e2e_engine` sends SIGUSR1 to. Clobbered, a `source e2e.sh` from a
# test aims that signal at the REAL e2e-test engine instead of the sandbox.
# Same convention as `e2e_lock.sh` (`${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}`).
# Nothing in the production path pre-sets it — `e2e-api.sh` / `e2e-browser.sh`
# only bare-`export` it after we assign — so the default is unchanged there.
E2E_WORKSPACE="${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}"

# Resolve paths: scripts/lib/e2e.sh → scripts/lib/ → scripts/ → project root
_E2E_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
_E2E_SCRIPTS_DIR="$(dirname "$_E2E_LIB_DIR")"
_E2E_PROJECT_DIR="$(dirname "$_E2E_SCRIPTS_DIR")"

# The e2e stack is deliberately allowed to run from a coding-agent worktree: a CC
# session's whole job is exercising the checkout it was invoked from, and the
# e2e-test workspace is disposable. Everything else refuses a worktree-rooted
# checkout (assert_stack_not_worktree_pinned in scripts/lib/workspace.sh), because
# a long-lived stack pinned to a throwaway copy serves a frozen engine binary +
# dist/ forever. This opt-in is the sanctioned exception, not a workaround.
export LUCIDOS_ALLOW_WORKTREE_STACK=1

# Test THIS checkout's engine-shipped knowhow, not whichever one the binary
# happens to sit under.
#
# The engine resolves `system-knowhow/` from `<repo_root>/system-knowhow`, and
# `repo_root` walks up from the running executable. The dev launcher publishes
# the engine binary to a launch dir shared by every workspace, which lives under
# the MAIN checkout, so a run started from a coding-agent worktree exercised the
# worktree's Rust against main's knowhow. Everything under `system-knowhow/` was
# therefore untestable by e2e until after it landed: the assertions passed or
# failed on a copy the branch had not touched.
#
# `LUCIDOS_SYSTEM_KNOWHOW_DIR` is the authoritative override (resolution rule 1
# in `core::system_knowhow`), and is what packaged builds already use. Pointing
# it at the checkout THESE SCRIPTS live in makes the knowhow half agree with the
# code half. Skipped when the dir is absent, which keeps the engine's own
# "set but missing is a packaging bug" warning meaningful.
if [ -z "${LUCIDOS_SYSTEM_KNOWHOW_DIR:-}" ] && [ -d "$_E2E_PROJECT_DIR/system-knowhow" ]; then
    export LUCIDOS_SYSTEM_KNOWHOW_DIR="$_E2E_PROJECT_DIR/system-knowhow"
fi

# Use mock LLM provider by default for e2e tests (override with LUCIDOS_MODEL=... before calling)
export LUCIDOS_MODEL="${LUCIDOS_MODEL:-mock}"

# E2E builds opt into the `e2e-test-hooks` cargo feature so the engine
# compiles in the push-log stub (replaces real web-push send with an
# in-process write) and the `GET /api/v1/_test/push-log` endpoint that
# Playwright tests assert against. See system-knowhow/notifications.md §5.4.
export ENGINE_BUILD_FEATURES="${ENGINE_BUILD_FEATURES:-e2e-test-hooks}"

# e2e runs on a RELEASE engine by default (docs/plans/2026-06-28-e2e-always-release-build.md).
# The debug engine's CPU cost is the dominant driver of the mobile-webkit
# WebContent cold-start contention wedge — running release eliminates that flake
# class and matches the packaged/prod engine (which IS release). The test-only
# `seed-change-for-test` endpoint stays reachable on release because it is gated on
# `cfg!(any(debug_assertions, feature = "e2e-test-hooks"))` and the e2e build passes
# that feature (ENGINE_BUILD_FEATURES above). `build_or_find_engine` reads $RELEASE.
#   - LUCIDOS_E2E_DEBUG=1 → fall back to the fast debug build for local single-spec
#     iteration (the opt-out is authoritative).
#   - otherwise RELEASE defaults to 1; an explicit caller RELEASE is honored.
if [ -n "${LUCIDOS_E2E_DEBUG:-}" ]; then
    export RELEASE=""
else
    export RELEASE="${RELEASE:-1}"
fi

# Cap cargo parallelism for the release compile so a full-core release codegen
# (wasmtime / aws-lc / ravif each eat 1–2 GB) can't blow past RAM into swap and
# hang the host (seen 2026-06-28 during this campaign). Half the cores by default;
# override with CARGO_BUILD_JOBS. Only applied to the release build path.
if [ -n "$RELEASE" ] && [ -z "${CARGO_BUILD_JOBS:-}" ]; then
    _e2e_cores="$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)"
    _e2e_jobs=$(( _e2e_cores / 2 ))
    [ "$_e2e_jobs" -lt 1 ] && _e2e_jobs=1
    export CARGO_BUILD_JOBS="$_e2e_jobs"
fi

# Source shared infrastructure — provides detect_tls, setup_postgres, start_engine,
# start_vite, etc. Set the globals workspace.sh expects from its caller.
PROJECT_DIR="$_E2E_PROJECT_DIR"
FRONTEND_DIR="$_E2E_PROJECT_DIR/crates/lucidos-app"
SCRIPT_NAME="e2e"

source "$_E2E_LIB_DIR/ports.sh"
source "$_E2E_LIB_DIR/workspace.sh"
source "$_E2E_LIB_DIR/e2e_lock.sh"
source "$_E2E_LIB_DIR/webkit_reaper.sh"
source "$_E2E_LIB_DIR/host_load_guard.sh"
source "$_E2E_LIB_DIR/host_memory_guard.sh"

# ── assert_e2e_workspace_is_disposable ──────────────────────────────────
# Refuse to proceed unless $E2E_WORKSPACE names a disposable e2e workspace.
#
# This suite DROPS the workspace's database and force-removes its worktrees and
# `lucidos-*` branches. `E2E_WORKSPACE` is deliberately env-overridable (the
# comment at the top of this file says why), and nothing checked what it
# pointed at. So `E2E_WORKSPACE=$HOME/workspaces/dev ./scripts/e2e.sh` dropped
# `lucidos_dev` and deleted that workspace's coding-agent worktrees.
#
# The name is the gate, matching the eval harness, which refuses a path whose
# name lacks its own prefix. Every sanctioned shape starts `e2e-`: the default
# `e2e-test`, and the sandboxes the lib tests pin before sourcing us
# (`$SANDBOX/e2e-test`, `$TMPROOT/e2e-ws`). A new sandbox must be named to match.
#
# Trailing slashes come off first. `${var##*/}` answers an empty string for a
# path written `.../e2e-test/`, which would refuse the default workspace.
assert_e2e_workspace_is_disposable() {
    local path="$E2E_WORKSPACE"
    while [ "${path%/}" != "$path" ]; do path="${path%/}"; done
    case "${path##*/}" in
        e2e-*) return 0 ;;
    esac
    echo "ERROR: E2E_WORKSPACE points at '$E2E_WORKSPACE'." >&2
    echo "       This suite drops that workspace's database and force-removes" >&2
    echo "       its worktrees, so it only runs against a disposable workspace" >&2
    echo "       whose directory name starts with 'e2e-'." >&2
    return 1
}

# Assert at SOURCE TIME, which is the only place that is actually safe.
#
# The two destructive functions call it too, but a refusal raised down there is
# too late: `setup_e2e_session` installs `trap teardown_e2e EXIT` BEFORE it
# calls them, and that teardown runs `stop_e2e_workspace` and
# `sweep_e2e_orphans`. Under the entry scripts' `set -e` the refusal aborts, the
# trap then fires against the workspace just refused, and the sweep SIGUSR1s its
# engine and kills every process under its worktrees. The guard would have
# triggered the destruction it exists to prevent.
#
# Here there is no trap, no lock and no consumer yet, and every later reader of
# `$E2E_WORKSPACE` in this library and in `e2e_lock.sh` is covered by
# construction rather than by remembering to add another call.
assert_e2e_workspace_is_disposable || exit 1

# ── e2e_workspace_env ───────────────────────────────────────────────────
# Resolve this workspace's globals (pidfiles, log path, PG_NAME), its ports and
# TLS. Idempotent and cheap. Both ensure_workspace_running and
# reset_e2e_database need it — the reset runs BEFORE the engine's first boot, so
# it can't rely on ensure_workspace_running having gone first.
e2e_workspace_env() {
    assert_e2e_workspace_is_disposable || return 1
    export WORKSPACE="$E2E_WORKSPACE"
    resolve_workspace
    allocate_ports "$WORKSPACE"
    detect_tls
}

# ── build_e2e_engine_once ───────────────────────────────────────────────
# Build the SDK bundle and the engine binaries the suite tests — ONCE per script
# invocation. Every database recreate restarts the engine (a fresh database is
# only migrated at boot), and each restart lands back here; recompiling
# mid-suite would swap the binary out from under the running tests, so later
# calls only LOCATE what the first call built. The marker is exported so the
# sub-scripts the umbrella (scripts/e2e.sh) spawns inherit "already built".
build_e2e_engine_once() {
    if [ -n "${LUCIDOS_E2E_ENGINE_BUILT:-}" ]; then
        BUILD=""
        build_or_find_engine
        return 0
    fi
    # Apps loaded in iframes fetch /api/v1/sdk.js — without dist/sdk.js the
    # engine serves a stub that lacks lucidos.ui/data, breaking SDK e2e tests.
    build_sdk
    BUILD="1"
    build_or_find_engine
    export LUCIDOS_E2E_ENGINE_BUILT=1
}

# ── frontend build: one shot, but never a STALE one ─────────────────────
# The e2e run tests a FIXED build — one `vite build` up front, then every project
# runs against exactly that dist/. That contract stays: the e2e path deliberately
# does NOT use the `vite build --watch`-style build-watch, which is a
# checkout-level singleton owned by the dev harness (start_frontend_built in
# scripts/lib/workspace.sh) and would swap the frontend out from under a running
# suite.
#
# What "one shot" must NOT mean is "any dist/ will do". The guard used to be
# existence-aware (`[ ! -f dist/index.html ]`), so a checkout whose dist/ predated
# its own frontend commits ran the WHOLE browser suite against a stale frontend
# and reported green — the harness lying about what it tested. It is now
# staleness-aware: rebuild when dist/index.html is missing OR older than any
# build input.
#
# Cheap by construction: ONE `find … -newer … -quit`, which stops at the first
# newer path instead of hashing the tree. mtime is trustworthy here because git
# stamps every file it writes with the checkout/merge time, so a checkout that
# moves frontend source forward always leaves those files newer than a dist/ built
# before it. The failure direction is an unnecessary rebuild, never a silently
# stale run.

# _frontend_build_inputs — echo one build-input path per line. Mirrors the watch
# list in crates/lucidos-app/dev-build-watch.mjs (the authoritative statement of
# what a `vite build` of this app reads), plus the files only vite.config.ts
# itself pulls in:
#   src/, public/, index.html, vite.config.ts  — the app and its entry points
#   tsconfig.json, package.json                — compiler options, deps, scripts
#   packages/lucidos-sdk/src/                  — the workspace-local package the
#                                                build aliases in as @lucidos/sdk
#   crates/lucidos-engine/VERSION              — baked into the bundle by the
#                                                engine-version plugin
#   <root>/package.json + package-lock.json    — npm workspaces hoist to the root
#                                                and `npm ci` restores node_modules
#                                                from the root lockfile, so a dep
#                                                bump changes the bundle without
#                                                touching a single app file. The
#                                                same two files _deps_fingerprint
#                                                keys the install on.
# Paths that don't exist are skipped by the caller, so an optional input costs
# nothing here.
_frontend_build_inputs() {
    printf '%s\n' \
        "$FRONTEND_DIR/src" \
        "$FRONTEND_DIR/public" \
        "$FRONTEND_DIR/index.html" \
        "$FRONTEND_DIR/vite.config.ts" \
        "$FRONTEND_DIR/tsconfig.json" \
        "$FRONTEND_DIR/package.json" \
        "$_E2E_PROJECT_DIR/packages/lucidos-sdk/src" \
        "$_E2E_PROJECT_DIR/crates/lucidos-engine/VERSION" \
        "$_E2E_PROJECT_DIR/package.json" \
        "$_E2E_PROJECT_DIR/package-lock.json"
}

# _first_build_input_newer_than REF — echo the first build-input path whose mtime
# is newer than REF, or nothing when the build is up to date.
#
# ALWAYS returns 0. The caller assigns this through a command substitution
# (`newer="$(…)"`), whose status IS the assignment's status — so under the
# callers' `set -e` a `find` that failed for any transient reason (a path removed
# between the -e probe and the walk, an unreadable dir) would abort the entire
# e2e run instead of just rebuilding. Failing open here means the worst case is a
# missed staleness signal, which the missing-dist branch and the next run still
# catch.
_first_build_input_newer_than() {
    local ref="$1"
    local p
    local -a inputs=()
    while IFS= read -r p; do
        [ -e "$p" ] && inputs+=("$p")
    done < <(_frontend_build_inputs)
    [ "${#inputs[@]}" -gt 0 ] || return 0
    find "${inputs[@]}" -newer "$ref" -print -quit 2>/dev/null || true
}

# _run_vite_build — the one-shot production build. Its own function purely so the
# shell test can redefine it (the repo's seam convention — see the measurement
# seams in scripts/lib/host_load_guard.sh) and assert WHICH branch
# ensure_frontend_built took without paying for a real build.
_run_vite_build() {
    (cd "$FRONTEND_DIR" && npx vite build)
}

# ensure_frontend_built — build dist/ when it is missing or stale, reuse it
# otherwise. ALWAYS prints the branch it took, so an unattended nightly log says
# in one line whether the suite tested a freshly built frontend or a reused one.
ensure_frontend_built() {
    local dist_index="$FRONTEND_DIR/dist/index.html"
    local reason="" newer=""

    if [ ! -f "$dist_index" ]; then
        reason="no dist/index.html"
    else
        newer="$(_first_build_input_newer_than "$dist_index")"
        if [ -n "$newer" ]; then
            reason="build input newer than dist/index.html: ${newer#"$_E2E_PROJECT_DIR"/}"
        fi
    fi

    if [ -z "$reason" ]; then
        echo "Frontend build: REUSED existing dist/ (newer than every build input)"
        return 0
    fi

    echo "Frontend build: REBUILDING dist/ (stale — $reason)"
    _run_vite_build || { echo "ERROR: frontend build failed" >&2; return 1; }
    echo "Frontend build: REBUILT dist/"
    return 0
}

# ── ensure_workspace_running ────────────────────────────────────────────
# Starts the e2e workspace if not running. Ensures both engine AND Vite are up.
# Uses LUCIDOS_MODEL=mock by default so tests don't hit real LLM APIs.
ensure_workspace_running() {
    e2e_workspace_env

    # After allocate_ports, VITE_PORT is the engine's port (see swap_ports).
    local engine_port="$VITE_PORT"

    # ── Frontend (ADR 0014: the engine serves the built dist/ directly) ──
    # No Vite dev server / proxy. swap_ports exports LUCIDOS_STATIC_DIR, and the
    # engine serves dist/ at / (base path '', no gateway). The e2e run tests a
    # fixed build, so a one-shot `vite build` suffices, as long as it is current
    # (see ensure_frontend_built).
    #
    # It runs BEFORE the engine starts, and that order is load-bearing. At boot
    # the engine pins the dist/ it finds into a private snapshot and serves THAT
    # (api/frontend_snapshot.rs), so a build landing afterwards never reaches a
    # test: the suite passes or fails on the PREVIOUS build, silently. It only
    # bites where dist/ is stale at boot, which is any worktree, since the
    # build-watch keeps the main checkout current. A reused engine (--no-reset)
    # already holds a pin, and only a restart re-takes it.
    ensure_frontend_built || return 1

    # ── Engine ──
    if curl -sk "${PROTO}://localhost:${engine_port}/api/v1/health" >/dev/null 2>&1; then
        echo "Engine already running on port $engine_port"
        # Set up env vars that swap_ports normally provides
        swap_ports
    else
        echo "Starting e2e workspace (LUCIDOS_MODEL=$LUCIDOS_MODEL)..."
        setup_postgres
        build_e2e_engine_once
        swap_ports
        start_engine
    fi

    # Final check: the engine must serve the built frontend at / (retry up to 30s)
    echo -n "Verifying engine serves the frontend"
    local frontend_ready=""
    # `for _`, never `for i`. e2e-browser.sh drives its project loop on `i` and
    # calls reset_e2e_database — which lands here — from inside that loop body,
    # so an un-localised counter named `i` leaked back into the caller and made
    # `PROJECT_RCS[$i]=$rc` write the WRONG slot: the last project's exit code was
    # never recorded (the blank "mobile:" line in the 2026-07-26 nightly summary)
    # and the previous project's was overwritten. A throwaway `_` can't leak
    # anything a caller could rely on; a named counter must be `local`. That is
    # enforced, not just asserted — e2e_test.sh's
    # test_no_sourced_lib_leaks_a_loop_variable scans every lib on this call path
    # and fails on a new leak.
    for _ in {1..30}; do
        if curl -sk "${PROTO}://localhost:${engine_port}/" 2>/dev/null | grep -q "<!DOCTYPE" 2>/dev/null; then
            echo " ready!"
            frontend_ready="yes"
            break
        fi
        echo -n "."
        sleep 1
    done

    if [ -z "$frontend_ready" ]; then
        echo ""
        echo "WARNING: Engine not serving the built frontend (is LUCIDOS_STATIC_DIR set / dist/ built?)"
    fi

    # Export for test scripts
    export VITE_PORT="$engine_port"
}

# Remove orphan dirs under $E2E_WORKSPACE/.lucidos/worktrees/ — directories
# with no .git pointer, or with a .git pointer to a gitdir that no longer
# exists. CC test sessions register worktrees in their spawning repo's
# .git/worktrees/; when that registration disappears (parent repo's worktree
# pruned first, partial cleanup, etc.) the directory remains. With dozens of
# leftover dirs the engine's startup recovery iterates over them and exceeds
# its 30s API readiness budget.
prune_orphan_worktree_dirs() {
    local wt_root="$E2E_WORKSPACE/.lucidos/worktrees"
    [ -d "$wt_root" ] || return 0

    local removed=0
    local d
    for d in "$wt_root"/*; do
        [ -d "$d" ] || continue
        if [ -z "$(ls -A "$d" 2>/dev/null)" ]; then
            rmdir "$d" 2>/dev/null && removed=$((removed + 1))
            continue
        fi
        if [ -f "$d/.git" ]; then
            local gitdir
            gitdir=$(sed -n 's/^gitdir: //p' "$d/.git" 2>/dev/null | head -1)
            if [ -n "$gitdir" ] && [ ! -d "$gitdir" ]; then
                rm -rf "$d" 2>/dev/null && removed=$((removed + 1))
            fi
        fi
    done
    [ "$removed" -gt 0 ] && echo "Pruned $removed orphan worktree dir(s)" || true
}

cleanup_e2e_worktrees() {
    assert_e2e_workspace_is_disposable || return 1
    echo "Cleaning up e2e worktrees..."
    local original_dir="$PWD"
    cd "$E2E_WORKSPACE" || return

    # `git worktree list` prints RESOLVED paths, so both comparisons below must
    # compare against a resolved $E2E_WORKSPACE or they silently never match —
    # and "never match" fails OPEN here: the workspace's own worktrees are left
    # registered in the canonical repo, which is exactly the stale-entry pileup
    # that pushes engine recovery past its 30s readiness budget. Bites whenever
    # the path traverses a symlink: a symlinked ~/workspaces, or (how this
    # surfaced) a macOS `mktemp -d` sandbox, where /var is a symlink to
    # /private/var.
    #
    # The empty-fallback below is LOAD-BEARING, not defensive boilerplate — do
    # not "simplify" it away (the `cd` above means resolution effectively can't
    # fail, which is exactly why it reads as dead code). An empty
    # $e2e_ws_resolved turns the `case` pattern into `/*`, which matches EVERY
    # absolute path, so the sweep below would `git worktree remove --force`
    # every worktree in the SHARED canonical repo and delete their branches —
    # the 2026-06-13 incident described in the SAFETY note further down. The
    # skip-main guard would break the same way. Fails CLOSED instead.
    local e2e_ws_resolved
    e2e_ws_resolved="$(cd "$E2E_WORKSPACE" 2>/dev/null && pwd -P)" || true
    [ -z "$e2e_ws_resolved" ] && e2e_ws_resolved="$E2E_WORKSPACE"

    # Prune stale worktree entries (paths that no longer exist on disk)
    git worktree prune 2>/dev/null

    # Remove all non-main worktrees (created by CC tests)
    local removed=0
    # Same rule as the readiness counter in ensure_workspace_running: this
    # function is called from inside e2e-browser.sh's project loop, so its
    # iteration variables must not leak back into the caller.
    local line
    while IFS= read -r line; do
        local wt_path
        wt_path=$(echo "$line" | awk '{print $1}')
        # Skip the main working tree
        [ "$wt_path" = "$e2e_ws_resolved" ] && continue
        git worktree remove --force "$wt_path" 2>/dev/null && removed=$((removed + 1))
    done < <(git worktree list 2>/dev/null)

    # Clean up leftover e2e-test branches. Safe to match by NAME here and only
    # here: this is the disposable e2e workspace's own git, which no real
    # session ever branches in. `lucidos-*` is the current coding-agent branch
    # prefix, `claude-code/*` the legacy one.
    git branch --list 'e2e-test/*' 'lucidos-*' 'claude-code/*' 'merge-tmp/*' 2>/dev/null | xargs -r git branch -D 2>/dev/null

    # CC test worktrees are physically inside this workspace but registered in
    # the canonical lucidos repo (where `git worktree add` ran). Without this the
    # repo accumulates stale entries every test run; engine recovery then iterates
    # over hundreds of dead worktrees on next startup and exceeds its 30s API
    # readiness budget.
    #
    # SAFETY — this repo is SHARED with every real CC session: dev/personal
    # worktrees and their `lucidos-*` branches all live here, and
    # `$_E2E_PROJECT_DIR` is whichever checkout invoked the script — frequently a
    # CC worktree of this same repo. So the ONLY safe discriminator for "created
    # by an e2e run" is the worktree path living under $E2E_WORKSPACE. NEVER
    # delete branches by name (`lucidos-*`, `claude-code/*`) or by ancestry: a just-started
    # real session has no commits ahead of main yet, so an ancestry sweep deletes
    # live user work — this force-deleted an active session's branch and wiped its
    # worktree on 2026-06-13. Delete ONLY the branch each removed e2e worktree was
    # checked out on, captured from the same `git worktree list` record.
    # Scoped as an `if` rather than an early return: not reaching the canonical
    # repo must skip ONLY this sweep. The cwd restore and the orphan-dir prune
    # below are independent of it and still have to run — returning here would
    # silently leave the caller in $E2E_WORKSPACE and skip the prune entirely.
    if cd "$_E2E_PROJECT_DIR" 2>/dev/null; then
        git worktree prune 2>/dev/null
        local wt_path="" cur_branch=""
        local -a e2e_branches=()
        while IFS= read -r line; do
            case "$line" in
                "worktree "*) wt_path="${line#worktree }" ;;
                "branch "*)   cur_branch="${line#branch refs/heads/}" ;;
                "")
                    case "$wt_path" in
                        "$e2e_ws_resolved"/*)
                            git worktree remove --force "$wt_path" 2>/dev/null && removed=$((removed + 1))
                            [ -n "$cur_branch" ] && e2e_branches+=("$cur_branch")
                            ;;
                    esac
                    wt_path=""; cur_branch=""
                    ;;
            esac
        done < <(git worktree list --porcelain 2>/dev/null; printf '\n')
        if [ "${#e2e_branches[@]}" -gt 0 ]; then
            local br
            for br in "${e2e_branches[@]}"; do
                git branch -D "$br" 2>/dev/null || true
            done
        fi
    fi

    # Warn rather than return: prune_orphan_worktree_dirs works on absolute
    # paths under $E2E_WORKSPACE, so it is still correct from any cwd and must
    # not be skipped just because the restore failed.
    cd "$original_dir" || echo "Warning: could not return to $original_dir" >&2
    [ "$removed" -gt 0 ] && echo "Removed $removed worktree(s)" || true

    prune_orphan_worktree_dirs
}

# ── stop_e2e_background_guards ───────────────────────────────────────
# Stop every run-scoped background helper the browser phase starts: the WebKit
# RSS reaper and the mid-run host-load sampler. Both are idempotent no-ops when
# nothing was started (e2e-api.sh starts neither), so this is safe in any
# teardown path. One function so a future guard gets wired into every teardown at
# once rather than being forgotten in one of them.
stop_e2e_background_guards() {
    stop_webkit_reaper
    stop_host_load_sampler
}

# ── playwright_file_filter ───────────────────────────────────────────
# Turn a spec path into a positional filter that selects exactly that file.
#
#   playwright_file_filter chat.spec.ts   # → /chat\.spec\.ts$
#
# Playwright reads a positional argument as an UNANCHORED, case-insensitive
# REGEX over the test file path, never as a filename. So a bare basename selects
# every sibling whose path merely CONTAINS it: `chat.spec.ts` also pulls in
# `app-coding-agent-spawn-from-chat.spec.ts`, `cancel.spec.ts` pulls in
# `coding-agent-cancel.spec.ts`, and `follow-ups.spec.ts` pulls in
# `coding-agent-follow-ups.spec.ts`.
#
# This is a correctness fix rather than a tidy-up. The selected specs ran twice
# and inflated the recorded pass count, and two of them spawn coding-agent
# subprocesses, so a run asked for three quiet specs got those as well.
# `scripts/e2e-browser.sh` routes its `-f <spec>` through here for that reason.
#
# The leading `/` is what makes the anchor a path boundary rather than a
# suffix: `chat\.spec\.ts$` alone still matches `…-chat.spec.ts`. Escaping
# covers every character that is not plainly literal in a regex, so a future
# spec name carrying `+` or `(` cannot reopen this.
playwright_file_filter() {
    local escaped
    escaped=$(printf '%s' "${1#./}" | sed 's|[^[:alnum:]/_-]|\\&|g')
    printf '/%s$' "$escaped"
}

# ── report_project_exit_codes ────────────────────────────────────────
# Print the per-project exit-code table for a multi-project browser run and
# RETURN the umbrella exit code the caller must exit with.
#
#   report_project_exit_codes OVERALL_RC PROJECT:RC [PROJECT:RC …]
#
# Lives here rather than inline in e2e-browser.sh so it is unit-testable — the
# thing it guards against is precisely a harness bug, and a guard with no test is
# the same bet that produced the bug.
#
# The guard: an entry whose rc is empty or non-numeric prints
# "UNKNOWN (harness bug)" and forces the umbrella exit code non-zero. A harness
# that cannot report a project's status must not report green — the 2026-07-26
# nightly printed a blank rc for `mobile` on a run where mobile had two real
# failures, and only the (independently computed) umbrella code kept that run
# honest. Nothing is masked in the other direction: a real non-zero overall_rc is
# passed through untouched.
# ── report_webkit_excluded ───────────────────────────────────────────
# Say that mobile-webkit was left out, given a non-empty first argument.
#
#   report_webkit_excluded "$SKIP_WEBKIT"
#
# mobile-webkit costs about 15 GB of macOS VM compressor and is run separately
# for that reason (docs/e2e-test-decisions.md). The project is DROPPED from the
# per-project table rather than recorded, so the table stays a record of what
# ran. That leaves the suite with a hole in it, and a run whose last words are
# "every project passed" is how such a hole goes unnoticed.
#
# Lives here beside report_project_exit_codes, and for the same reason: what it
# guards against is a harness misreport, and a guard with no test is the bet
# that produced the misreport.
report_webkit_excluded() {
    [ -n "${1:-}" ] || return 0
    echo ""
    echo "[e2e] mobile-webkit was EXCLUDED by --no-webkit and did NOT run."
    echo "[e2e] Coverage is incomplete until it runs on its own, on a cold host:"
    echo "[e2e]   ./scripts/e2e-browser.sh --webkit"
}

report_project_exit_codes() {
    local overall="$1"; shift
    local entry name rc unknown=""

    # A non-integer overall is itself a harness bug, and `return` would reject it.
    case "$overall" in
        ''|*[!0-9]*)
            echo "ERROR: umbrella exit code '$overall' is not an integer (harness bug) — forcing 1" >&2
            overall=1
            ;;
    esac

    echo ""
    echo "── Per-project exit codes ──"
    for entry in "$@"; do
        name="${entry%%:*}"
        rc="${entry#*:}"
        case "$rc" in
            ''|*[!0-9]*)
                echo "  $name: UNKNOWN (harness bug)"
                unknown=1
                ;;
            *)
                echo "  $name: $rc"
                ;;
        esac
    done

    if [ -n "$unknown" ]; then
        echo ""
        echo "ERROR: the harness could not determine every project's exit code."
        echo "       A run whose per-project status is unknown must not report green —"
        echo "       forcing a non-zero exit code."
        [ "$overall" -eq 0 ] && overall=1
    fi

    return "$overall"
}

# ── summarise_playwright_log ─────────────────────────────────────────
# Add up every Playwright summary in a log and echo one tally line:
#
#   planned passed failed flaky skipped interrupted didnotrun invocations
#
# A project is run in chunks, so it prints a summary per invocation and never a
# figure for itself. That is what "chunked green is not green" names: nobody adds
# them up, so nothing notices an invocation whose result went missing. This is
# the adding up, and `report_playwright_totals` below is the noticing.
#
# `planned` comes from the "Running N tests" banner and is the CONTROL. Every
# planned test lands in exactly one outcome bucket, retries included: a test that
# fails then passes is reported once, as flaky. So the buckets must sum to the
# banner, and an invocation that died before printing its summary breaks that sum
# rather than passing silently.
#
# Colour is stripped first. Playwright emits SGR codes when stdout is a terminal,
# and a `\033[32m` before the digits would leave every count at zero.
summarise_playwright_log() {
    awk '
        { gsub(/\033\[[0-9;]*m/, "") }
        /^Running [0-9]+ tests? using/ { planned += $2; invocations++ }
        /^ +[0-9]+ passed/            { passed += $1 }
        /^ +[0-9]+ failed/            { failed += $1 }
        /^ +[0-9]+ flaky/             { flaky += $1 }
        /^ +[0-9]+ skipped/           { skipped += $1 }
        /^ +[0-9]+ interrupted/       { interrupted += $1 }
        /^ +[0-9]+ did not run/       { didnotrun += $1 }
        END {
            printf "%d %d %d %d %d %d %d %d",
                planned, passed, failed, flaky, skipped,
                interrupted, didnotrun, invocations
        }
    ' "$1" 2>/dev/null
}

# ── report_playwright_totals ─────────────────────────────────────────
# Print ONE verdict line for a project run across many invocations, and RETURN
# non-zero when the outcomes do not account for every planned test.
#
#   report_playwright_totals <project> <log>
#
# The return is a harness verdict, never a test one: the caller folds it in with
# merge_rc so it can only ever ADD a failure. A project whose chunks do not add
# up has not been measured, and must not read green for the same reason
# report_project_exit_codes refuses to print a blank rc.
report_playwright_totals() {
    local project="$1" log="$2"
    local tally planned passed failed flaky skipped interrupted didnotrun invocations accounted

    if [ ! -s "$log" ]; then
        echo ""
        echo "ERROR: no Playwright output was captured for $project (harness bug)."
        return 1
    fi

    tally="$(summarise_playwright_log "$log")"
    # shellcheck disable=SC2086 # deliberate word split: the tally is eight fields
    set -- $tally
    planned="${1:-0}"; passed="${2:-0}"; failed="${3:-0}"; flaky="${4:-0}"
    skipped="${5:-0}"; interrupted="${6:-0}"; didnotrun="${7:-0}"; invocations="${8:-0}"
    accounted=$(( passed + failed + flaky + skipped + interrupted + didnotrun ))

    echo ""
    echo "── $project total: $planned tests over $invocations invocation(s) ──"
    echo "   $passed passed, $failed failed, $flaky flaky, $skipped skipped," \
        "$interrupted interrupted, $didnotrun did not run"

    if [ "$invocations" -eq 0 ]; then
        echo ""
        echo "ERROR: $project printed no Playwright summary at all (harness bug)."
        return 1
    fi
    if [ "$planned" -ne "$accounted" ]; then
        echo ""
        echo "ERROR: $project planned $planned tests but accounted for $accounted."
        echo "       An invocation ended without reporting, so this run is not a"
        echo "       verdict. Forcing a non-zero exit code."
        return 1
    fi
    return 0
}

# ── kill_orphan_simulator ────────────────────────────────────────────
# The Simulator's Virtualization VM survives `simctl shutdown` (the XPC service
# persists for fast reboot) and holds multiple GB resident, so reclaim it.
#
# CRITICAL: prove the VM is the Simulator's before signalling it. Docker
# Desktop drives the same framework, and its helper is the identical Apple
# binary with an EMPTY argv, reparented to launchd. Neither a name match nor a
# ppid check can tell the two apart, so `pkill -f` on that binary killed
# Docker's VM, taking `lucidos-pg-shared` and every workspace's database with
# it. Gating on CoreSimulatorService did not help: it proves a Simulator is
# alive, never whose VM this is. The open files are the honest discriminator.
# A VM we cannot place is LEFT ALONE, because leaked RAM is recoverable and a
# dropped database cluster is not.
kill_orphan_simulator() {
    pgrep -x Simulator >/dev/null 2>&1 || pgrep -f "com.apple.CoreSimulator.CoreSimulatorService" >/dev/null 2>&1 || return 0
    xcrun simctl shutdown all >/dev/null 2>&1 || true
    killall Simulator 2>/dev/null || true
    command -v lsof >/dev/null 2>&1 || return 0
    local vm_pid vm_comm
    ps -Aww -o pid=,comm= 2>/dev/null | while read -r vm_pid vm_comm; do
        case "$vm_pid" in ''|*[!0-9]*) continue ;; esac
        [ "$vm_pid" -le 1 ] && continue
        [ "${vm_comm##*/}" = "com.apple.Virtualization.VirtualMachine" ] || continue
        # -n -P: no reverse DNS and no port-name lookup. This runs inside the
        # e2e lock window, and a bare lsof on a multi-GB VM resolves every
        # INET fd it holds.
        lsof -n -P -p "$vm_pid" 2>/dev/null | grep -q CoreSimulator || continue
        kill "$vm_pid" 2>/dev/null || true
    done
}

# ── setup_e2e_session ────────────────────────────────────────────────
# Standard sub-script lifecycle: lock, an EXIT trap teardown that mirrors the
# reset choice, and then either a database reset (which brings the workspace up
# on a brand-new database) or, under --no-reset, a plain ensure_workspace_running
# on whatever is already there.
# When invoked under the umbrella ($LUCIDOS_E2E_UMBRELLA set), defers all
# of that to the umbrella and only refreshes port globals.
# NO_RESET is read from the caller's env (sub-scripts already parse --no-reset).
#
# Usage:
#   setup_e2e_session <lock-label>
#       Skip cleanup_e2e_worktrees on teardown (api default).
#   setup_e2e_session <lock-label> --cleanup-worktrees-on-teardown
#       Browser tests can leave CC worktrees behind; clean them on exit too.
setup_e2e_session() {
    local label="$1"
    local cleanup_on_teardown=""
    case "${2:-}" in
        "") ;;
        --cleanup-worktrees-on-teardown) cleanup_on_teardown=1 ;;
        *) echo "setup_e2e_session: unknown option '$2'" >&2; exit 1 ;;
    esac

    if [ -n "${LUCIDOS_E2E_UMBRELLA:-}" ]; then
        # Umbrella owns lock + workspace + initial reset; we just need port globals.
        ensure_workspace_running
        return 0
    fi

    acquire_e2e_lock "$label" || exit 1
    kill_orphan_simulator

    # stop_e2e_background_guards leads every branch so the host-memory reaper and
    # the mid-run host-load sampler started by e2e-browser.sh die with the
    # session. Both are idempotent no-ops when nothing was started (e.g.
    # e2e-api.sh), so it's safe in all branches.
    # shellcheck disable=SC2329 # all three variants are invoked by `trap teardown_e2e EXIT` below
    #
    # `sweep_e2e_orphans` comes AFTER `stop_e2e_workspace` in both stopping
    # variants: the coding-agent subprocesses the tests spawned are the engine's
    # children, so they are only leftovers once the engine is gone. The
    # `--no-reset` variant deliberately skips it, since that branch leaves the
    # workspace RUNNING for the next invocation and its agents are not orphans.
    if [ -n "${NO_RESET:-}" ]; then
        # Leave the workspace running so the next invocation starts immediately
        # instead of paying the boot cost again.
        teardown_e2e() { stop_e2e_background_guards; release_e2e_lock; }
    elif [ -n "$cleanup_on_teardown" ]; then
        teardown_e2e() {
            stop_e2e_background_guards
            cleanup_e2e_worktrees
            stop_e2e_workspace
            sweep_e2e_orphans
            release_e2e_lock
        }
    else
        teardown_e2e() {
            stop_e2e_background_guards
            stop_e2e_workspace
            sweep_e2e_orphans
            release_e2e_lock
        }
    fi
    # Installed BEFORE the workspace comes up so a failure during the reset still
    # releases the lock.
    trap teardown_e2e EXIT
    trap 'exit 130' INT TERM

    if [ -n "${NO_RESET:-}" ]; then
        ensure_workspace_running
    else
        cleanup_e2e_worktrees
        # Recreates the database from zero and boots the workspace on it. The
        # engine must start AFTER the recreate to run the migration chain, so the
        # reset owns the boot — don't add an ensure_workspace_running before it.
        reset_e2e_database
    fi
}

stop_e2e_workspace() {
    echo "Stopping e2e workspace..."
    "$_E2E_SCRIPTS_DIR/stop.sh" -w "$E2E_WORKSPACE" 2>/dev/null || true
    # ADR 0014: no long-lived frontend process to stop — the engine serves the
    # built dist/ directly and the one-shot `vite build` exits on its own.
}

# ── stop_e2e_engine ─────────────────────────────────────────────────────
# Stop this workspace's engine and wait for it to release its port. SIGUSR1, not
# SIGTERM — the engine ignores SIGTERM so an accidental `xargs kill` from a CC
# subprocess test can't take it down (main.rs shutdown_signal). SIGUSR1 is the
# legitimate stop, and it also ends the supervisor's restart loop, so the engine
# stays down until we start it again. No-op when nothing is running.
stop_e2e_engine() {
    local pid=""
    if [ -f "$ENGINE_PIDFILE" ]; then
        pid="$(cat "$ENGINE_PIDFILE" 2>/dev/null || true)"
    fi
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        echo "Stopping engine (PID $pid) so the database can be recreated..."
        kill -USR1 "$pid" 2>/dev/null || true
        wait_for_engine_shutdown "$VITE_PORT" "$pid" || true
    fi
    rm -f "$ENGINE_PIDFILE"
}

# ── reset_e2e_database ──────────────────────────────────────────────────
# Bring the e2e database up EXACTLY like a brand-new workspace's: drop it,
# recreate it, and boot the engine on it so sqlx runs the ENTIRE migration chain
# from zero — the seeds inside those migrations included.
#
# This replaces a TRUNCATE of every table except `_sqlx_migrations`, which made
# the database long-lived rather than fresh: the surviving `_sqlx_migrations`
# rows told sqlx every migration was already applied, so their seeds never
# re-ran and any table whose only content is a migration seed stayed
# permanently empty. `models` was the casualty — 0 rows instead of 26, so
# `llm::model_registry::load_from_db` built an empty map and provider routing
# silently fell back to the prefix heuristic for every model.
#
# It owns the engine lifecycle, and has to: Postgres refuses to drop a database
# that still has open connections, and migrations, `EventStore::init_schema()`
# and the pgvector setup all run exactly once, at engine boot (see
# engine/engine_impl/construction.rs) — so the engine serving the tests must be
# the one started AFTER the recreate. On return the workspace is running on a
# genuinely fresh database.
#
# Deliberately NOT creating the `vector` extension here: a brand-new workspace
# database doesn't have it either (the engine creates it at boot in
# memory/pgvector.rs, and no migration uses a pgvector type). Adding it would
# make the e2e database differ from the thing it is supposed to reproduce, and
# would hide a migration that starts depending on the extension.
reset_e2e_database() {
    e2e_workspace_env
    setup_postgres

    stop_e2e_engine

    local db
    db="$(workspace_database_name)"
    echo "Recreating database $db from zero..."
    _drop_shared_database "$db" || return 1
    _create_shared_database "$db" || return 1

    # `psql -c` without ON_ERROR_STOP exits 0 even when the statement errored, so
    # a refused DROP (something still connected) would silently leave the OLD
    # database in place and quietly restore the very bug this replaces. Assert
    # the outcome instead of trusting the exit code.
    local leftover
    leftover=$(docker exec "$(shared_pg_container)" psql -U lucidos -d "$db" -At -c \
        "SELECT to_regclass('_sqlx_migrations') IS NOT NULL;")
    if [ "$leftover" != "f" ]; then
        echo "ERROR: $db was not recreated — _sqlx_migrations is still present." >&2
        echo "       Something still holds a connection to it." >&2
        return 1
    fi

    # Boots the engine, which runs the whole migration chain against the empty
    # database — seeds included.
    ensure_workspace_running
}
