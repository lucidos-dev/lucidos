---
paths:
  - "scripts/web-dev.sh"
  - "scripts/run.sh"
  - "scripts/tauri-dev.sh"
  - "scripts/stop.sh"
  - "scripts/restart.sh"
  - "scripts/status.sh"
  - "scripts/tail.sh"
  - "scripts/populate.sh"
  - "scripts/new-migration.sh"
  - "scripts/merge-drivers.sh"
  - "scripts/replay-merge-drivers.sh"
  - "scripts/lib/merge_drivers*.sh"
  - "scripts/decommission-legacy-postgres.sh"
  - "scripts/dev-codesign-setup.sh"
  - "scripts/dev-refresh-app-frontend.sh"
  - "scripts/deps-state.sh"
  - "scripts/lib/deps_state*.sh"
  - "scripts/test-engine.sh"
  - "scripts/codegen.sh"
  - "scripts/lib/codegen*.sh"
  - "scripts/harden-gates.sh"
  - "scripts/harden-suites.sh"
  - "scripts/lib/harden_suites*.sh"
  - "scripts/harden-scope.sh"
  - "scripts/lib/harden_scope_test.sh"
  - "scripts/lane-report.sh"
  - "scripts/**/*other*model*review*.sh"
  - "scripts/lib/proc_tree.sh"
  - "scripts/test-scripts.sh"
  - "scripts/with-build-slot.sh"
  - "scripts/lib/with_build_slot_test.sh"
  - "scripts/with-run-limit.sh"
  - "scripts/lib/with_run_limit_test.sh"
  - "scripts/build-memory-gate.sh"
  - "scripts/measure-engine-build.sh"
  - "scripts/engine-module-graph.py"
  - "scripts/build-openssl-prebuilt.sh"
  - "scripts/lint-shell.sh"
  - "scripts/lib/lint_shell_test.sh"
  - "scripts/check-em-dashes.sh"
  - "scripts/check-prose.sh"
  - "scripts/lib/em_dash_scan*.sh"
  - "scripts/lib/prose_scan*.sh"
  - "scripts/check-adrs.sh"
  - "scripts/check-context-budget.sh"
  - "scripts/check-knowhow-refs.sh"
  - "scripts/check-audit-scans.sh"
  - "scripts/check-retired-terms.sh"
  - "scripts/lib/check_retired_terms_test.sh"
  - "scripts/check-default-flows.sh"
  - "scripts/lib/check_default_flows_test.sh"
  - "scripts/check-prompt-mirror.sh"
  - "scripts/check-build-script-paths.sh"
  - "scripts/check-eval-not-a-test.sh"
  - "scripts/eval-context-mode.sh"
  - "scripts/eval-comments/**"
  - "scripts/lib/build_script_path_scan*.sh"
  - "scripts/adr-new.sh"
  - "scripts/lib/adr_scan*.sh"
  - "scripts/lib/context_budget*.sh"
  - "scripts/lib/prompt_mirror*.sh"
  - "scripts/lib/hooks_registered_test.sh"
  - "scripts/e2e*.sh"
  - "scripts/main-check.sh"
  - "scripts/lib/main_check*.sh"
  - "scripts/lib/workspace*.sh"
  - "scripts/lib/pg_secret*.sh"
  - "scripts/lib/ports*.sh"
  - "scripts/lib/storage_root*.sh"
  - "scripts/lib/flock*.sh"
  - "scripts/lib/build_watch_exec.pl"
  - "scripts/lib/e2e*.sh"
  - "scripts/lib/engine_manifest*.sh"
  - "scripts/lib/touched_specs*.sh"
  - "scripts/lib/gateway_supervisor*.sh"
  - "scripts/lib/engine_supervisor*.sh"
  - "scripts/lib/codesign.sh"
  - "scripts/lib/docker*.sh"
  - "scripts/lib/preflight.sh"
  - "scripts/lib/preflight_test.sh"
  - "scripts/lib/engine_build_only_test.sh"
  - "scripts/lib/engine_build_spares_other_builds_test.sh"
  - "scripts/lib/stale_objects*.sh"
  - "scripts/lib/preflight_gate_test.sh"
  - "scripts/lib/proc_env*.sh"
  - "scripts/lib/sleep.sh"
  - "scripts/lib/host_load_guard*.sh"
  - "scripts/lib/host_memory_guard*.sh"
  - "scripts/memory-watch*.sh"
  - "scripts/lib/memory_watch*.sh"
  - "scripts/preflight-reclaim-engines.sh"
  - "scripts/lib/preflight_reclaim*.sh"
  - "scripts/lib/webkit_reaper*.sh"
  - "scripts/lib/sigterm_contract_test.sh"
  - "scripts/lib/wait_for_engine_shutdown_test.sh"
  - "scripts/lib/em_dash*.sh"
  - "Makefile"
---

# Dev Runtime & Workspace Scripts

Launching a workspace, dev topology, frontend serving and the engine test harness. Build and packaging: `.claude/rules/build-release.md`. Facts in no ADR or header: [`docs/dev-runtime.md`](../../docs/dev-runtime.md).

**`paths:` lists scripts explicitly.** Add each new script here or to `build-release.md`, or neither rule loads.

## Dev / runtime scripts

**Run these from the real checkout, never from a coding-agent worktree** (ADR 0021). A worktree copy of `scripts/` resolves `PROJECT_DIR` to the throwaway checkout. It pins the gateway, engine and served `dist/` there, so every frontend-only Apply silently does nothing.

- `assert_stack_not_worktree_pinned` (`scripts/lib/workspace.sh`) refuses with the corrective command. The gateway refuses a worktree-rooted `LUCIDOS_ENGINE_BIN`, self-reload and spawn-env passthrough.
- `LUCIDOS_ALLOW_WORKTREE_STACK=1` (set by `scripts/lib/e2e.sh`) covers only a session-scoped direct engine. The gateway ignores it: that daemon is machine-global, and `-b` relaunches it from the invoking checkout.
- **`./scripts/web-dev.sh -w e2e-test -b` from a worktree is refused.** Don't reach for `LUCIDOS_NO_GATEWAY=1`. The pre-start is redundant: **just run `./scripts/e2e.sh`** (or `e2e-api.sh` / `e2e-browser.sh`). `ensure_workspace_running` in `scripts/lib/e2e.sh` provisions everything.

Each script's header is its manual. Daily ones: `web-dev.sh -w <ws> [-b] [-r]` (dev start), `run.sh -w <ws>` (user start), `tauri-dev.sh`, `stop.sh -w <ws>`, `status.sh`, `new-migration.sh`, `test-engine.sh [--full|--fresh]`, `test-scripts.sh`, `lint-shell.sh`, `with-build-slot.sh -- <cmd>` (ADR 0070), `preflight-reclaim-engines.sh`, `memory-watch.sh --once` (ADR 0264). `main-check.sh <sha>` is the engine's (ADR 0464): never run it by hand. The `check-*.sh` gates and `harden-*.sh` helpers are documented in their headers.

### The two evals spend money and run by hand only

`scripts/eval-comments/eval-comments.sh` and `scripts/eval-context-mode.sh` (ADR 0110, about $120 per run) SPEND MONEY.

- **Nothing runs them from `make test`, `/harden` or a workflow.** `check-eval-not-a-test.sh` fails `make lint` if `cargo test` can reach one (ADR 0087).
- **An eval touches only workspaces it created** (`eval-<arm>-<repeat>`). Its pins are the measurement: changing one changes what a result means.
- **No criterion may name an internal of the mode** (`Fixture::validate`, ADR 0110 decision 5).

### Pre-flight reclaim, memory watch, Docker probe

The libraries `preflight_reclaim.sh`, `memory_watch.sh` and `docker.sh` carry the rationale in their headers.

- **Stop an engine with `scripts/stop.sh`, never a raw signal.** The engine ignores SIGTERM, and the gateway respawns one that gets a bare SIGUSR1.
- **Read a workspace path from the process environment** (`ps -E`), never from an assumed layout. A survivor or a comeback fails the reclaim (ADR 0179).
- **Never kill what you cannot name.** The memory watch measures footprint, never RSS, and records before it kills.
- **The Docker probe is an exit status, never a matched error string.** `classify_docker_probe` mirrors `docker_daemon_state` in `crates/lucidos-gateway/src/postgres.rs`: **keep the two in step** (ADR 0037).
- **Every `scripts/lib/*_test.sh` is hermetic by construction** (ADR 0025). Read a suite's stubs before running it alone.

### Shell lint (`make lint-shell`) and Rust formatting (`make lint-fmt`, ADR 0030)

- **Discovery is `git ls-files '*.sh'`.** A missing ShellCheck or an empty discovery fails the gate. Flags live in the root `.shellcheckrc`.
- **Suppress only a genuine false positive, with the reason on the same line.** Prefer `# shellcheck source=<path>`. Verify an SC2034 or SC2329 first, since some are dead code.
- **macOS bash is 3.2.** No `mapfile`, and `${#arr[@]}` on an empty array trips `set -u`.
- **There is no `rustfmt.toml`, and `lint-fmt` has no `--locked`.** `cargo fmt` rejects the flag. The fix is `make fmt`. Generated Rust lives in `OUT_DIR`, never the tree.

### Workspace gateway + dev topology (ADR 0014: dev is not packaged)

**Two ports are live at once.** Read ADR 0014 § "Dev runtime topology" before touching ports or binds.

| | binds | serves |
|---|---|---|
| **engine** (one per workspace) | `ENGINE_PORT` = `VITE_PORT` (5173+offset), loopback only, plain http | the workspace app at `/` |
| **gateway** (ONE per machine) | fixed **5251** (`LUCIDOS_DEV_GATEWAY_PORT`) | `https://localhost:5251/<slug>/`, the picker at `/~/`, and a proxy to every launched workspace |

Dev uses 5251, not 5252: 5252 is the packaged `Lucidos.app` gateway, so both coexist.

**The gateway is the only network door, in dev as well as packaged.**

- **Dev engines bind loopback**, and the gateway proxies over the engine's Unix socket (ADR 0445). A network-bound engine port walks past pairing (ADR 0094). `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0` reopens that bypass.
- **A directly launched engine is not exempt.** `apply_dev_engine_bind` pins `LUCIDOS_BIND_ADDR=127.0.0.1` for e2e and otherwise sets nothing. Widening is deliberate.
- **A wide engine bind asks for a credential** (`api::local_auth`, ADR 0155), and `api::browser_origin` refuses a cross-origin browser request. `LUCIDOS_PERMISSIVE_CORS` turns off the second and NOT the first.
- **Register a directory by POSTing `/~/api/v1/control/workspaces/adopt`**, never by writing `workspaces.json` (ADR 0438). Contract: `docs/dev-runtime.md`.
- **The shared Postgres password is minted, never `lucidos`.** `~/.lucidos/pg-shared-password` (0600) holds it, and it never goes in argv.
- **Probe through the workspace prefix** (`/<slug>/api/v1/...`): `docs/dev-runtime.md`.
- **`stop.sh -w <ws>` leaves the gateway up.** `engine_gateway_port` reads which gateway owns the engine off its environment.
- **e2e drives a direct engine** (`scripts/lib/e2e.sh`), pinned with `LUCIDOS_PIN_ENGINE_VERSION=1`, on a RELEASE build by default (ADR 0022, ADR 0385). A local filtered run takes the debug engine (ADR 0536). `LUCIDOS_NO_GATEWAY=1` runs the legacy single-engine model.

**The `web-dev.sh -w <ws>` sequence** (`scripts/lib/workspace.sh`): `swap_ports`, `build_or_find_engine`, `start_gateway`, `adopt_into_gateway`, `ensure_workspace_engine_running`. **Auto-start, gateway self-reload and the legacy Postgres cluster:** `docs/dev-runtime.md`.

### A launch never tears down a healthy engine (ADR 0219)

**A dev launch adopts a running engine. Only `-b` and `--engine-only` replace a live one.** A launcher teardown is unattributed, so every in-flight thread settles as an `engine_shutdown` abort and none auto-resume.

- **Ownership of a port is what the engine ANSWERS, never a pidfile** (`engine_on_port_serves_workspace`, `scripts/lib/ports.sh`). A timeout is not an answer: only a refused connection says nothing is there.
- **A test that stubs a library function must RESTORE it** (`restore_reclaim_fn`), never `unset -f` it.

**A pidfile belongs to its owner, under its owner lock (ADR 0438).** Neither supervisor writes one.

- **A script trusts `engine.pid` only while `engine.lock` is held** (`engine_live_pid`, `scripts/lib/flock.sh`) and never deletes it. Signal through `engine_pids_to_signal`, which also checks the pid still runs `lucidos-engine`.
- **Never delete a `.lock` file.** A new opener would lock a new inode. Probe a lock only where its owner acquires blocking, so nothing probes `gateway.lock`.
- `run_gateway_supervised` is `disown`ed and ignores SIGTERM. Its only stop is SIGUSR1 to the gateway child.

### Published launch binaries (ADR 0022 + 0063): never launch from `target/<profile>/lucidos-engine`

Every cargo variant uplifts to that path, and the last writer wins. A completed `build_or_find_engine` publishes the engine, gateway and `lucidos` CLI into `.launch/<profile>/<variant>/`, and every launch path points there.

- **Atomic, never destructive.** Copy to a temp name, then `mv -f`.
- **Verified against HEAD** (`published_build_state`). A mismatch asks the binary (`--build-id --source-state`), so a CSS-only merge costs no second build.
- **`cargo clean` must never reach `.launch/`.** Staying inside the checkout is load-bearing, staying inside `target/` is not.
- **`crates/lucidos-e2e` must never request `features = ["e2e-test-hooks"]`** on its engine dev-dependency. Cargo would unify it at workspace scope.
- **Signing follows the published binary** (`sign_engine_binary`), so macOS TCC grants survive a rebuild (`scripts/dev-codesign-setup.sh`).

**Apply builds in the background, then switches** (ADR 0412, `docs/plans/2026-07-01-new-engine-version-switch-flow.md`). The switch only respawns onto the built binary, and a second Apply joins the build in flight. A built engine must boot on a copy of the workspace database before it is published. An engine build never signals another cargo or deletes a `.cargo-lock` (ADR 0294). `--engine-build` allocates no ports.

### Frontend: the engine serves the built `dist/` directly (ADR 0014)

`start_frontend_built` runs the build-watch (`crates/lucidos-app/dev-build-watch.mjs`): a fresh `vite build` in a clean child process on every change. No Vite sits in the serving path, and there is no HMR. The script header holds the rest.

- **The running engine serves ONLY a client compatible with itself (INV-A).** At boot it pins a private snapshot of `dist/` (`api/frontend_snapshot.rs`). A *Switch to new version* respawns it, so client and engine advance together.
- **Exception: a frontend-only Apply advances the served client in-process** (`engine/frontend_refresh.rs`, `docs/plans/2026-07-02-frontend-only-apply-served-in-dev.md`), gated on `frontend_advance_is_safe`.
- **Peers catch up on their own**, guarded by `engine_source_matches_head`, which reuses Apply's `files_require_restart` (`docs/plans/2026-07-03-cross-workspace-frontend-only-refresh.md`).
- **`dist/` publishes atomically**, so a failed build keeps the last good one and cannot 404 the app.
- **Shared build-watch: one per checkout**, under three locks in `crates/lucidos-app/.build-watch/` (ADR 0438). A running watch is reused, so stop it to pick up an edit to `dev-build-watch.mjs`. Teardown is one non-blocking try on `consumers.lock`.
- **`ensure_npm_deps` never installs under a live build-watch.** Under `ENGINE_ONLY` and `ENGINE_BUILD_ONLY` it warns and returns 0, never a hard fail. It is `npm ci`, never `npm install`: `.claude/rules/build-release.md` § "Lockfile determinism".
- **`.build-watch/status.json` records the last build, and `watch.lock` says whether anything still builds.**
- **The service worker caches `/assets/*` first and the shell network-first** (`networkFirstShell`), because a cache-first shell once blacked out the PWA.
- A missed update toast: `docs/dev-runtime.md` § Debugging a missed toast.

### Port allocation never signals an ancestor (ADR 0025)

`allocate_ports` can reclaim a port from a stale engine with `kill -USR1`. In `is_protected_host_pid`, two arms the caller cannot defeat: never this process or an ancestor, and never pid 1 or below.

- **The allocation is one critical section** (ADR 0438): an exclusive flock on `$LUCIDOS_PORT_REGISTRY.lock`.
- **Sandboxing `HOME` does NOT disarm host protection.** A test needing a protected pid uses `start_lock_holder` (`scripts/lib/flock_test_support.sh`).
- **A process-selection guard matches `argv[0]`, NEVER the whole command line.** A mention of a path is not a process running from it, and the difference is a SIGKILL. `webkit_reaper.sh` and `e2e_lock.sh` match `${command%% *}`. The e2e sweep also needs its run marker (ADR 0251).
- **A `scripts/lib/*_test.sh` that sources `ports.sh` stubs `lsof` for the WHOLE file.** It also needs a kill shim refusing any lethal signal to a pid it did not spawn. Copy `ports_test.sh`.
- **A process-listing seam fails CLOSED.** An empty synthetic feed means "no candidates", never the real `ps`.

## Engine tests need Postgres: use `test-engine.sh`

Engine integration tests (`setup_test_db`, `crates/lucidos-engine/src/test_support.rs`) create a throwaway `lucidos_test_*` database per test and read `TEST_DATABASE_URL`.

`make test` runs the eval, gateway and library crates, then `test-engine.sh`. `make test-full` runs the whole crate. A filter goes after `--`: `./scripts/test-engine.sh -- -- migration_tests`. `--fresh` recreates the test container.

`test-engine.sh` provisions a disposable `lucidos-pg-test` container (`pgvector/pgvector:pg18`, port `LUCIDOS_TEST_PG_PORT`, default 5510), isolated from every workspace's PG. Restart with `web-dev.sh -b`. The shared Postgres stays up when one workspace stops.
