---
paths:
  - "scripts/web-dev.sh"
  - "scripts/run.sh"
  - "scripts/tauri-dev.sh"
  - "scripts/start.sh"
  - "scripts/stop.sh"
  - "scripts/restart.sh"
  - "scripts/status.sh"
  - "scripts/logs.sh"
  - "scripts/tail.sh"
  - "scripts/populate.sh"
  - "scripts/new-migration.sh"
  - "scripts/decommission-legacy-postgres.sh"
  - "scripts/dev-codesign-setup.sh"
  - "scripts/dev-refresh-app-frontend.sh"
  - "scripts/test-engine.sh"
  - "scripts/lint-shell.sh"
  - "scripts/check-em-dashes.sh"
  - "scripts/e2e*.sh"
  - "scripts/lib/workspace*.sh"
  - "scripts/lib/ports*.sh"
  - "scripts/lib/e2e*.sh"
  - "scripts/lib/gateway_supervisor*.sh"
  - "scripts/lib/engine_supervisor*.sh"
  - "scripts/lib/codesign.sh"
  - "scripts/lib/preflight.sh"
  - "scripts/lib/sleep.sh"
  - "scripts/lib/host_load_guard*.sh"
  - "scripts/lib/webkit_reaper*.sh"
  - "scripts/lib/sigterm_contract_test.sh"
  - "scripts/lib/wait_for_engine_shutdown_test.sh"
  - "scripts/lib/em_dash*.sh"
  - "Makefile"
---

# Dev Runtime & Workspace Scripts

Launching a workspace, gateway + dev topology, how the frontend is served, and
the engine test harness. The build / packaging / installer half of the former
`scripts.md` is `.claude/rules/build-release.md`.

**`paths:` above is an explicit list, not a `scripts/**` catch-all** — that's
what makes a build-script edit skip this file. A new script under `scripts/`
therefore gets NO rule until its path is added here or to `build-release.md`.

## Dev / runtime scripts

**Run these from the real checkout — never from a coding-agent worktree.** A worktree
is a complete copy of the repo, `scripts/` included, so `PROJECT_DIR="$(dirname
"$SCRIPT_DIR")"` silently resolves there and pins the gateway binary, engine binary and
served `dist/` to a throwaway checkout frozen at one commit. The build-watch then
republishes the *real* checkout's `dist/`, which that stack never reads, so every
frontend-only Apply silently does nothing (the 2026-07-26 incident — ADR 0021).
`assert_stack_not_worktree_pinned` (`scripts/lib/workspace.sh`) refuses with the
corrective command; the gateway refuses a worktree-rooted `LUCIDOS_ENGINE_BIN`, self-reload,
and spawn-env passthrough. The opt-out `LUCIDOS_ALLOW_WORKTREE_STACK=1` (set by `scripts/lib/e2e.sh`) applies **only
to a session-scoped direct engine**, and is **ignored for the gateway** — that daemon is
machine-global, `disown`ed, and `-b` relaunches it from whatever checkout invoked it, so a
worktree-rooted gateway outlives the session and serves every workspace a frozen `dist/`.
Consequence: **`./scripts/web-dev.sh -w e2e-test -b` from a worktree is refused.** Don't
reach for `LUCIDOS_NO_GATEWAY=1` to get around it — that only drops to `stack` scope, which
still needs `LUCIDOS_ALLOW_WORKTREE_STACK=1` that `web-dev.sh` does not set. The pre-start
is redundant: **just run `./scripts/e2e.sh`** (or `e2e-api.sh` / `e2e-browser.sh`).
`ensure_workspace_running` in `scripts/lib/e2e.sh` already does `setup_postgres` +
`build_e2e_engine_once` + `swap_ports` + `start_engine` + a one-shot `vite build`, sets the
opt-in itself, and never starts a gateway.

```bash
./scripts/web-dev.sh -w <ws> [-b] [-r]    # DEV start (-b builds engine+gateway; -r release; engine serves built dist/; vite watch)
./scripts/run.sh -w <ws>                  # USER start (installer entry point): release engine + one-shot vite build, no watcher
./scripts/tauri-dev.sh -w <ws> [-b]       # Start engine + Tauri window
./scripts/stop.sh -w <ws>                 # Stop a specific workspace
./scripts/status.sh                       # Check running status
./scripts/populate.sh -w <ws> [-c]        # Populate test history
./scripts/new-migration.sh <description>  # Create timestamped migration
./scripts/dev-codesign-setup.sh           # One-time: stable macOS code-signing identity
./scripts/dev-refresh-app-frontend.sh [-a <app>] [--no-build] [--restart]  # macOS: rebuild dist + sync into an installed .app's Resources/frontend + re-seal (fast frontend-only loop; native path is inert in tauri dev so the packaged app is the only place to test it)
./scripts/test-engine.sh [--full|--fresh] # Engine tests against a dedicated Docker PG
./scripts/e2e-packaged.sh [--rebuild]     # macOS-only: boot the packaged .app (service + embedded PG) and smoke-test the chain (heavy: builds the .app)
./scripts/lint-shell.sh                   # ShellCheck over every tracked *.sh (= make lint-shell; part of make lint / make check)
./scripts/check-em-dashes.sh [--base <ref>]  # Fail if the branch ADDS a U+2014 / U+2015 (diff-scoped; /harden Phase 4.5 runs it for every diff). Rule + rationale: .claude/rules/no-em-dashes.md
```

### Shell lint (`make lint-shell`)

Every tracked `*.sh` in the repo is ShellCheck-clean and stays that way: `lint`
depends on `lint-shell`, `lint-fmt` and `lint-rust`, so `make check` covers it,
and `/harden` Phase 4.5 runs `make lint` for any diff touching `*.sh`,
`.shellcheckrc`, the `Makefile`, or Rust, so the gate holds on every change
rather than only when someone thinks to run it.

- **Discovery is `git ls-files '*.sh'`, not a path list.** Every shell script here
  carries a `.sh` extension (no extensionless ones), so the glob is exact — and a
  script added in ANY directory is linted the day it is committed. This deliberately
  avoids the maintenance trap the `paths:` lists above have, where a rule that
  silently fails to cover a new file looks exactly like a rule that doesn't exist.
- **Flags live in the tracked root `.shellcheckrc`, not in the invocation** —
  `external-sources=true` + `source-path=SCRIPTDIR`, which make ShellCheck resolve a
  relative `source` against the sourcing script's own directory, the way these
  scripts resolve it at runtime. Keeping them there means a bare
  `shellcheck scripts/foo.sh`, an editor's inline ShellCheck, and the gate all reach
  the same verdict. It is also what keeps the check honest: without it ShellCheck
  can't see across a `source`, and reports a pile of phantom "unused variable" /
  "unreachable function" findings that tempt you into suppressing real checks.
- **Missing ShellCheck fails the gate** (non-zero + install hint), never skips — a
  disarmed gate must not read as clean. Same for a discovery that returns nothing.
- **Suppress only a genuine false positive, and give the reason on the same line**
  (`# shellcheck disable=SC2034 # read by scripts/web-dev.sh after it calls this parser`).
  For a `source` ShellCheck can't resolve, prefer `# shellcheck source=<path>` over a
  disable — the directive keeps the sourced file analysed. Verify before suppressing
  an SC2034/SC2329: some are genuinely dead and should be deleted (CLAUDE.md).
- Site-local suppressions are explicitly OUT of `docs/temporary-measures.md`, so they
  need no registry row.
- **`mapfile` does not exist on macOS bash 3.2**, and `${#arr[@]}` on an EMPTY array
  trips `set -u` there. `lint-shell.sh` reads its file list with a `while read` loop
  and counts as it goes for exactly that reason.

### Rust formatting (`make lint-fmt`, ADR 0030)

Every tracked `.rs` file is rustfmt-clean and stays that way. `lint-fmt` runs
`cargo fmt --all --check` and fails with a pointer at `make fmt`; the tree was
brought clean in one mechanical sweep first (424 of 614 files at the time).

- **The fix is `make fmt`**, which takes the same `--all` as the check so the
  advertised remediation covers exactly what the gate inspects.
- **There is deliberately no `rustfmt.toml`.** Stock defaults are reproducible
  because `rust-toolchain.toml` pins the toolchain and rustfmt with it. A config
  file would be a footgun: on a stable channel rustfmt WARNS and continues on a
  nightly-only key rather than failing, so an inert setting reads as an active
  one. That same nightly-only limitation is why the `ignore` key cannot be used
  to exclude a path.
- **This one cargo call carries no `--locked`**, against ADR 0020's blanket rule.
  `cargo fmt` rejects the flag outright and resolves no dependencies, so there is
  no lockfile for it to drift against. Do not "fix" the inconsistency.
- **Generated Rust must be emitted already-formatted.** A tracked generated file
  is squeezed between its staleness test (bytes must equal the emitter's output)
  and this gate (bytes must be rustfmt-clean), and it cannot be excluded. See
  `capability_manifest/codegen.rs`, which pipes its output through `rustfmt`.
- **A toolchain bump can now reformat the tree**, so a `rust-toolchain.toml`
  channel bump may have to carry a sweep. See `.claude/rules/build-release.md`
  § "Toolchain pin".

### Workspace gateway + dev topology (ADR 0014 — Dev ≠ packaged!)

**Dev runtime topology is two ports, both live at once** (ADR 0014 §4 + the normative "Dev runtime topology" table — read it before touching ports/binds):

| | binds | URL | serves |
|---|---|---|---|
| **engine** (one per workspace) | `ENGINE_PORT` = `VITE_PORT` (5173+offset), **all interfaces** | `https://localhost:5173/` | the workspace app at `/` (base `/`) — directly, exactly as the pre-gateway engine did |
| **gateway** (ONE per machine) | `GATEWAY_PORT` = **fixed 5251** (override `LUCIDOS_DEV_GATEWAY_PORT`) | `https://localhost:5251/<slug>/` + picker `…/~/` | proxies `/<slug>/` to each engine; serves the picker listing **every** launched workspace. Dev uses **5251**, NOT 5252 — 5252 is the packaged `Lucidos.app` gateway, so dev + packaged coexist out of the box |

**Loopback-only is the PACKAGED posture, NOT dev.**

- Dev engine binds all interfaces on its own port, so `https://localhost:5173/` reaches it directly; the gateway spawns it that way (`LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0`) and proxies + health-probes over **https** (self-signed cert accepted). Never make the dev engine loopback-only — breaks direct access, contradicts §4.
- The gateway ITSELF also binds all interfaces in dev via `LUCIDOS_GATEWAY_BIND_ALL=1` (set by `start_gateway`, sibling opt-in to `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0`). It defaults to loopback-only as its packaged security posture, so dev must opt in explicitly — otherwise a gateway rebuild+reload returns on `127.0.0.1` only, unreachable for the picker + `/<slug>/` routing from other devices (e.g. an iOS PWA over Tailscale).
- Packaged (`desktop.rs::spawn_gateway`, `LUCIDOS_PACKAGED=1`) does NOT run `start_gateway` → stays loopback-only.

**`web-dev.sh -w <ws>` sequence** (`scripts/lib/workspace.sh`):

1. `swap_ports` — `ENGINE_PORT=VITE_PORT` (per-workspace); `GATEWAY_PORT=5251` (**fixed, shared**; override `LUCIDOS_DEV_GATEWAY_PORT`).
2. `build_or_find_engine` — builds, **publishes** and signs BOTH `lucidos-engine` and `lucidos-gateway` (plus the `lucidos` CLI). See "Published launch binaries" below: the launch path is `target/<profile>/launch/<variant>/`, NOT cargo's shared `target/<profile>/lucidos-engine`.
3. `seed_gateway_registry` — upserts this workspace into the **machine-global** registry `$HOME/.lucidos/gateway/config/workspaces.json` (NOT per-workspace): refreshes its **direct** engine port + workspace dir, removes any legacy `database_url`, **preserves** any picker-set display name + `autostart` flag. A brand-new entry defaults `autostart:false`.
4. `start_gateway` — reuses a healthy gateway already on 5251, else starts ONE under the **dedicated gateway supervisor** `run_gateway_supervised` (`scripts/lib/gateway_supervisor.sh`) with `LUCIDOS_ENGINE_BIN=<engine>`, `LUCIDOS_STATIC_DIR=<dist>`, `LUCIDOS_API_PORT=5251`, `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0`, `LUCIDOS_GATEWAY_BIND_ALL=1`, `LUCIDOS_GATEWAY_DATA=$HOME/.lucidos/gateway`, `LUCIDOS_GATEWAY_PG_BACKEND=docker`, `LUCIDOS_GATEWAY_PG_PORT=<shared-pg-port>`, `LUCIDOS_GATEWAY_PG_CONTAINER=lucidos-pg-shared`.
5. POSTs `/~/api/v1/control/workspaces/<id>/restart` to start (or respawn, for Apply) THIS workspace's engine — needed because new workspaces default `autostart:false`, so the gateway's own boot won't spawn them.

`run_gateway_supervised` is NOT the engine's `run_supervised`: the gateway is a machine-global daemon, so its supervisor does `trap '' SIGHUP SIGINT SIGTERM` and is launched `disown`ed, surviving the launching `web-dev.sh` shell + terminal close (the way the packaged Rust `--service` survives under launchd `KeepAlive`). Its only legitimate stop is SIGUSR1 to the gateway child.

**Postgres:** one shared Docker container/volume (`lucidos-pg-shared` / `lucidos-pg-data-shared`), one database per workspace (`lucidos_<slug>`). Legacy per-workspace containers are migration sources only until explicitly decommissioned.

**Routing:** the gateway reverse-proxies `/<slug>/` as a pure streaming forward — strips `/<slug>`, adds `X-Forwarded-Prefix: /<slug>/`, forwards the response untouched (no body rewrite). Gateway-owned surface (picker, control API, health) lives behind the reserved **sigil namespace `/~/`**; `/` smart-redirects into the sole workspace, or serves the picker when there are several. The picker lists **every workspace ever launched** (durable membership); a stopped one stays listed and **lazy-starts** on a proxy hit / explicit open. Workspaces created from the picker get their Docker Postgres provisioned by the gateway (container `lucidos-pg-gw-<id>`).

**Probing a served asset through the gateway needs the workspace prefix.** Hit `https://<host>/<slug>/api/v1/...`, NOT a bare `https://<host>/api/v1/...` — the gateway resolves the FIRST path segment as a workspace slug, so a bare `/api/v1/...` reads as workspace `api` and 404s with `unknown workspace 'api'`. (Hitting an engine directly on its own port is base `/`, so `…:<port>/api/v1/…` works there — the gotcha is gateway-only.)

### Published launch binaries (ADR 0022) — never launch from `target/<profile>/lucidos-engine`

`target/<profile>/lucidos-engine` is **one output path every cargo variant in the checkout uplifts to**, and the last writer wins: a workspace-scope `cargo test`, an e2e `--features e2e-test-hooks` build, a build whose `build.rs` ran before an Apply moved HEAD. Launching from it meant a workspace could run — and could read back through `current_exe --build-id` — a binary from another commit or another feature configuration (the 2026-07-26 downgrade/toast loop; `docs/plans/2026-07-27-launch-binary-published-per-variant.md`).

So a completed `build_or_find_engine` **publishes** `lucidos-engine` + `lucidos-gateway` + the `lucidos` CLI into

```
target/<profile>/launch/<variant>/     # <profile> = debug|release, <variant> = plain | e2e-test-hooks | …
```

and `ENGINE_BIN` / `GATEWAY_BIN` / `LUCIDOS_ENGINE_BIN` / the engine's `current_exe()` all point there. That directory is written **only by completed builds of the same profile AND feature variant** — a dev workspace (`debug/launch/plain`) and the e2e harness (`release/launch/e2e-test-hooks`, or `debug/launch/e2e-test-hooks` under `LUCIDOS_E2E_DEBUG=1`) are structurally disjoint. Cargo keeps uplifting to `target/<profile>/lucidos-engine`; **nothing launches from it.**

- **Atomic, never destructive.** Copy to a temp name in the destination dir, then `mv -f`. A failed publish leaves the previously published binary byte-identical and removes the temp — a build must never make the launch path missing (`No engine binary found. Run with -b` would strand every co-located workspace). The no-build path prefers the published launch binary and **falls back to cargo's uplift path with a warning** when there isn't one yet.
- **Verified against HEAD.** After publishing, `published_build_state` compares the binary's build-id commit prefix with `git rev-parse --short HEAD` (prefix-matched in both directions — the two sides abbreviate to different lengths). `stale` ⇒ rebuild **once**; still stale ⇒ warn and succeed. No git / unreadable id / a `src-…` id classify `unknown` and never trigger a rebuild.
- **Signing follows the launched binary.** `sign_engine_binary` runs on the published copies. The dev Designated Requirement is identifier + certificate leaf (no CDHash, no path), so macOS TCC grants survive the move.
- **Staying under `target/` is load-bearing**, not incidental: the engine resolves the checkout by walking `current_exe()`'s ancestors for `scripts/web-dev.sh` (`paths::repo_root` — `run_engine_build`, the shared build lock and `engine_source_matches_head` all need it), and ADR 0021's worktree refusal is a pure path test on `LUCIDOS_ENGINE_BIN`. A workspace-local staging dir would break the first and launder a worktree binary past the second.
- **`crates/lucidos-e2e` must never request `features = ["e2e-test-hooks"]`** on its `lucidos-engine` dev-dependency. It is a *dev*-dependency, so cargo unifies the feature at workspace scope and a bare `cargo test` then builds + uplifts a hooks-enabled engine (push transport stubbed, `/api/v1/_test/*` exposed). The hooks belong to the engine BINARY the harness builds via `ENGINE_BUILD_FEATURES`.

**Stopping:** `stop.sh -w <ws>` does NOT kill the shared gateway — it POSTs `/~/api/v1/control/workspaces/<id>/stop` (gateway drops that stack so its supervisor won't respawn it; the registry entry survives, so the workspace stays listed) and leaves the gateway up for peers. Stop the gateway itself with `kill $(cat $HOME/.lucidos/gateway/gateway.pid)`.

- **Standalone crate (ADR 0014 §1):** the gateway is `crates/lucidos-gateway/` with NO dependency on `lucidos-engine` — the only network-facing process links proxy + supervise + registry code, not the engine's heavy core. It spawns the engine by path via `LUCIDOS_ENGINE_BIN` (its own `current_exe` is the gateway).
- **Engine serves the frontend directly:** both the gateway (picker) and every spawned engine serve the built `dist/` from `LUCIDOS_STATIC_DIR`. The engine stamps `<base href="/<slug>/">` into `index.html` from `X-Forwarded-Prefix` (default `/` when hit directly) so relative asset refs resolve back through the gateway. **No Vite in the serving path** (no `dev_proxy`, no `vite preview`).
- **Apply → background build, then switch** (`docs/plans/2026-07-01-new-engine-version-switch-flow.md`): *Apply* is non-disruptive. For an engine-affecting change (dev), the engine kicks off a BACKGROUND rebuild via `web-dev.sh --engine-build` (build-only: `build_or_find_engine` + `build_sdk`; NO kill/respawn/Vite) while the running engine keeps serving; a second Apply coalesces (aborts + restarts the build). When the on-disk binary's `ENGINE_BUILD_ID` differs from the running one, the frontend surfaces "New version available → Switch to new version" (`GET /api/v1/engine/version-status` poll). The **switch** (`/api/v1/restart`) only RESPAWNS onto the already-built binary — no build at switch: gateway dev/packaged POSTs `/~/api/v1/control/workspaces/<id>/restart` (gateway SIGUSR1s + respawns the engine, peers untouched); legacy `LUCIDOS_NO_GATEWAY` dev falls back to `web-dev.sh --engine-only` (fast near-noop build then respawn); packaged without a gateway uses launchd. Boundary "Switched to new version" events are emitted at ACTUAL teardown by `main.rs::shutdown_signal`, never during the build. A full `-b` still stops the gateway so a rebuilt gateway binary is used.
- **Consistent version signal + self-heal** (`docs/plans/2026-07-03-engine-version-switch-selfheal.md`). The "New version available" surface used to be driven ONLY by `version-status.update_available` (on-disk binary build-id ≠ running). Dead-end when the background rebuild **fails or never completes** (e.g. the concurrent-`target/`-build failure below): binary stays stale ⇒ `update_available` stays false ⇒ NO Switch — while the frontend-only-Apply INV-A veto (`engine_source_matches_head`) simultaneously and correctly defers every frontend-only Apply to that never-arriving Switch, so all co-located workspaces serve stale JS with no actionable UI. Two fixes:
  - **Consistent signal.** `version-status` also reports **`source_behind_head`** — engine SOURCE is behind HEAD by a restart-requiring change, via the SAME `engine_source_matches_head` git classifier the veto uses, so a new engine version is discoverable before a fresh binary exists. TTL-cached (`engine_version::source_behind_head`, `SOURCE_BEHIND_TTL`) so the `git diff` runs at most once per interval regardless of client count. Frontend surfaces it as a "New engine version pending — Rebuild" toast (`checkEngineVersion`, gated on `!update_available` so it never nags once a switchable binary exists), plus a "Retry build" action on the build-failed toast. `POST /api/v1/engine/rebuild` is the manual trigger behind both (no-op packaged). The pending-Rebuild toast is ALSO gated on `!shared_build_in_progress` (see peer-build spinner below).
  - **Self-heal.** The dev periodic loop (`frontend_refresh::spawn_served_frontend_sync`, ~10s) runs `self_heal_engine_version_if_needed`: source behind + stale on-disk binary + no build in flight ⇒ (re)trigger a background rebuild, so the Switch surfaces WITHOUT a manual `-b`. **Bounded** — `SELF_HEAL_MAX_ATTEMPTS_PER_HEAD` per HEAD (reset when HEAD moves) so a broken `main` can't spin builds forever; the build-failed toast stays surfaced. **Coordinated** — co-located workspaces share ONE checkout + ONE `target/`, so `run_engine_build` holds a checkout-shared advisory **build lock** (`fs2` flock at `<repo_root>/target/.lucidos-engine-build.lock`, auto-released on drop / process death): exactly one `web-dev.sh --engine-build` runs at a time; others get `EngineBuildOutcome::SkippedLocked` (→ `build_state` back to `Idle`, NOT `Failed`) and observe the shared binary advance. Upholds the "never two concurrent cargo builds on the same target" rule (CLAUDE.md) — that collision was the likely original cause of the wedge. Scope: the lock serializes only *engine-triggered* builds (Apply, self-heal, `POST /engine/rebuild`), which fire automatically en masse. A human `web-dev.sh -b` is a deliberate single action and is NOT lock-coordinated — macOS ships no `flock` binary, so shell-side locking is out of scope.
  - **Multi-workspace peer-build spinner** (`docs/plans/2026-07-04-multi-workspace-peer-build-spinner.md`). The `SkippedLocked` → `Idle` fallback created a UX gap: the workspace that LOST the shared lock (a bystander, OR the very workspace that clicked Apply if a peer grabbed the lock first) sat in `source_behind_head && !update_available && build_state == idle` and showed the manual "New engine version pending — Rebuild" toast even though a peer's build was in flight and WOULD advance the shared binary. Its self-heal also skipped (`engine_build_in_progress_elsewhere()` sees the peer's lock), so it was NOT short-lived. Fix: `version-status` reports **`shared_build_in_progress`** — the checkout-shared build lock is held (this engine's build or a peer's), from the same cheap non-blocking `engine_build_in_progress_elsewhere()` flock probe (no TTL cache — it forks no process, unlike `source_behind_head`). `checkEngineVersion` then lights the building spinner (`engineBuilding`) when `shared_build_in_progress && source_behind_head && !update_available && build_state == idle`, and gates the pending-Rebuild toast on `!shared_build_in_progress`, so the manual escape hatch appears only when NOTHING is building and the workspace is genuinely stuck. Once any build lands, `update_available` flips → the normal ready→Switch surface (the spinner's peer disjunct is gated on `!update_available`, so it hands off cleanly).
- **Gateway self-reload (picker reload control):** the `--engine-only` Apply restart leaves the shared gateway on its already-compiled binary, so a change to `crates/lucidos-gateway/**` (e.g. boot-splash HTML) is rebuilt on disk but NOT served until the gateway restarts. `GET /~/api/v1/control/gateway/status` returns `{build_id, update_available, packaged}` — the running process's baked `GATEWAY_BUILD_ID` (git short SHA + hash of any uncommitted gateway-source diff, baked by `crates/lucidos-gateway/build.rs`; printable via `lucidos-gateway --build-id`) and whether the on-disk binary is **NEWER** — not merely different (behind a cheap `current_exe` mtime gate so the picker's 2s poll doesn't fork per tick). Direction is decided the same way the engine decides it, by git ancestry over the two ids' commit prefixes (`crates/lucidos-gateway/src/build_id.rs`, a hand-synced copy of the engine's `engine_version.rs` helpers — ADR 0014 §1 forbids linking the engine; **keep the two in step**). Without it, `reload_gateway`'s re-exec would walk the machine's only gateway BACKWARDS onto an older binary another build left in `target/`. The picker shows a reload icon, badged when `update_available`. `POST /~/api/v1/control/gateway/reload` makes the gateway **re-exec itself** onto the on-disk binary (`execv(current_exe, argv)`): SAME PID, so the supervisor keeps `wait`ing (no respawn) and `gateway.pid` stays valid; the fresh `main()` re-adopts running engines. This is the ONLY in-place gateway restart — distinct from the supervisor's SIGUSR1, which is the gateway's *permanent* stop (clean exit → supervisor stops, `scripts/lib/gateway_supervisor.sh`). The endpoint returns 202 before the short-delayed exec so the picker's request resolves. **DEV-ONLY:** `packaged` is `true` under the packaged runtime (`desktop.rs::spawn_gateway` sets `LUCIDOS_PACKAGED=1`; dev's `web-dev.sh` sets nothing → `false`) and the picker renders the icon only when `!packaged` — a packaged build never rebuilds in place; its updates go through the app updater + a full launchd service restart (`crates/lucidos-app/src/updater.rs`, `docs/desktop-app.md`).
- **Auto-start + boot (ADR 0014):** the registry's per-workspace `autostart` flag (picker toggle → `POST /~/api/v1/control/workspaces/<id>/autostart {enabled}`) governs gateway boot — it **re-adopts** already-running engines, **spawns** auto-start workspaces, leaves the rest **stopped** (lazy-started on open). New dev workspaces default `autostart:false`: an explicit `web-dev.sh` launch starts them for the session (via the restart POST) but they won't auto-start on a future gateway boot until toggled on. There is no auto-created `default`: on an empty registry the gateway creates nothing and the smart root serves the picker. Hence the dev launcher always POSTs restart rather than relying on gateway boot.
- **Shared Postgres (ADR 0014 §6/§7):** the dev launcher starts/verifies one shared Docker Postgres cluster and ensures the launched workspace database exists. If a legacy per-workspace `lucidos-pg-<cksum>` cluster exists and the shared database is not verified, it dumps/restores the old `lucidos` database into `lucidos_<slug>`, verifies the target, writes a marker under `.lucidos/`, and leaves the old container/volume intact. Remove legacy data only with `./scripts/decommission-legacy-postgres.sh -w <ws>`, which refuses without the marker and a reachable shared database.
- **Escape hatch:** `LUCIDOS_NO_GATEWAY=1 ./scripts/web-dev.sh -w <ws>` runs the legacy single-engine model (no gateway); the engine serves the app at `/` with base `/`. Separate mode from the dev engine's direct access above, which coexists with the gateway.
- **e2e** drives the legacy direct-engine model (`scripts/lib/e2e.sh` calls `start_engine` directly with `LUCIDOS_STATIC_DIR` set + a one-shot `vite build`); frontend served at `/` (base path `''`).

### Frontend: the engine serves the built `dist/` directly (ADR 0014)

`start_frontend_built` runs the build-watch (`crates/lucidos-app/dev-build-watch.mjs`): an initial `vite build`, then a **fresh `vite build` in a clean child process on every source change**, producing the bundled `dist/` that the engine serves via `LUCIDOS_STATIC_DIR`. No `vite preview`, no engine→Vite proxy. Each build stamps a fresh `BUILD_ID` into `sw.js` and the same id into the app bundle as `CLIENT_BUILD_ID` via the `virtual:build-id` module (the `lucidos-sw-stamp` plugin in `crates/lucidos-app/vite.config.ts`). Trade-off: **no HMR**; builds are sub-second.

**Service-worker caching:** content-hashed `/assets/*` are **cache-first** (immutable by hash), so a reload pulls the heavy JS/CSS graph from disk. The navigation shell (`index.html`) is **network-first** (`networkFirstShell` in `sw.js`), falling back to the cached shell only offline → fast iOS PWA resume / notification-tap reload without the ~10s cold-load black screen over Tailscale, while keeping the shell in lockstep with the assets the rebuilt server has. (The shell was cache-first once: a long-lived PWA pinned a stale `index.html` referencing `/assets/*` bundles a later `vite build --watch` had deleted, the SPA fallback served those as `text/html`, the entry module failed to parse, and the PWA went all-black — `system-knowhow/notifications.md` §4.5 thirteenth iteration.)

**The running engine serves ONLY a client compatible with itself — never a newer one, not even on reload.** In dev the engine does NOT serve the live `dist/`: at boot it takes a private **pinned snapshot** (`<workspace>/.lucidos/served-frontend/<generation>/`, hardlink-copy, numbered subdir per snapshot; `crates/lucidos-engine/src/api/frontend_snapshot.rs`) behind a **swappable handle** (`Arc<RwLock<PathBuf>>`) and serves THAT (`serve_frontend` reads the current generation per request). The build-watch keeps advancing the shared `dist/`, but the running engine keeps serving the client it was built against — so a hard reload can NEVER pull a newer, possibly-engine-incompatible client onto the old engine. **Load-bearing invariant (INV-A):** a new endpoint / event / migration in a mixed change would break the old-engine + new-client pairing. A *Switch to new version* respawns the engine; the new process snapshots the then-current `dist/`, so client and engine advance together.

**Exception — a frontend-only Apply advances the served client in-process** (no respawn; `crates/lucidos-engine/src/engine/frontend_refresh.rs`, `docs/plans/2026-07-02-frontend-only-apply-served-in-dev.md`). A pure frontend change (`files_require_restart == false`) leaves the engine binary unchanged, so a newer client built from that diff IS compatible. The applying engine waits for the build-watch to republish `dist/` (polls source `sw.js` `BUILD_ID` vs the served snapshot's, bounded timeout), pins a **fresh generation**, and atomically swaps the handle — served `sw.js` advances and the client refresh badge/toast fire without a restart. A **mixed** change still advances only via a Switch. **Gated on the running engine binary being current** (`build_state == Idle` AND on-disk binary id matches the running one — `frontend_advance_is_safe`): if a mixed change was applied but not yet switched, `dist/` already holds a client built for the NEW engine, so a later frontend-only Apply must NOT snapshot it onto the still-old engine. The gate is checked before the poll AND again before the swap (a mixed Apply can land mid-poll). **On the deferred branch the engine emits the transient `FrontendUpdateDeferred` event** (`engine::frontend_refresh::emit_frontend_update_deferred`) so the page surfaces a keyed "frontend change applies on Switch" hint toast (`store/actions/engine-update.ts` `handleFrontendUpdateDeferred`) instead of the Apply appearing to do nothing. Coalesced (a later frontend-only Apply supersedes an in-flight refresh); fail-safe (a failed re-snapshot leaves the current one in place — never a 404); the superseded generation is removed after a grace delay so an in-flight request never 404s.

**Packaged** serves its immutable bundled Resources directly (already one unit — no snapshot; the in-process refresh is a no-op there); a failed boot snapshot falls back to serving the live dir (never a 404).

**Peers catch up on their own.** A shared-`dist` rebuild by one workspace does not immediately change what a *peer* engine serves (each has its own snapshot; the in-process refresh runs only in the applying engine). A dev-only per-engine periodic task (`engine::frontend_refresh::spawn_served_frontend_sync`, ~10s) re-snapshots the shared `dist/` and emits the transient `ServedFrontendAdvanced` event **only when advancing is INV-A-safe**. "Safe" is NOT just `disk == running`: during ANOTHER workspace's *mixed*-change rebuild the on-disk binary stays old for tens of seconds while the build-watch has already republished `dist/` with a new-engine client, so the disk gate alone would drag the peer onto an incompatible client. The load-bearing guard is `engine_source_matches_head`, which classifies files changed since the running engine's commit (`git diff --name-only <running-engine-commit> HEAD`) with the SAME `files_require_restart` classifier the Apply path uses: no restart-requiring file ⇒ frontend-only ⇒ advance; a restart-requiring file ⇒ mixed change in flight ⇒ defer to *Switch* (the peer gets the Switch badge from the shared binary). Reusing that exact classifier — not a coarse `crates/lucidos-engine` pathspec — is what keeps the gate from stranding a frontend-only change that also touches a restart-IGNORED engine file (a test `.rs`, a `.md`). The same git veto hardens the applying engine's own frontend-only path against a concurrent peer mixed rebuild. See `docs/plans/2026-07-03-cross-workspace-frontend-only-refresh.md`.

**Badge ⟺ toast — coupled on ARRIVAL, decoupled on DISMISS.** Because the served client is always engine-compatible, `syncClientUpdateFromBuild` (running bundle's `CLIENT_BUILD_ID` vs served `sw.js` `BUILD_ID`) is an honest "is my loaded code stale?" signal, true *only* when a compatible newer client is actually served.

- **Badge** ("client update available" dot) = `updateAvailable = stale` — the persistent affordance.
- **Toast** ("New version available — refresh to sync") surfaces when `stale && !wasSwUpdateDismissed(served)`.
- On ARRIVAL both appear together — a lit badge is never alone *on arrival*. They decouple only on DISMISS: the toast's X or its **"Later"** action hides the toast and remembers this build, but the badge stays lit so the user can still refresh from it. Dismissal is **durable** (`localStorage`, keyed by build id — survives reload AND cold relaunch); a genuinely newer served build re-surfaces the toast.
- There is **no engine-pending gate** on the toast — the serving layer, not the toast, upholds "never a client for a non-running engine".
- The engine **Switch** surface mirrors this: badge = `engineVersionReady = ready`; toast ("New version available → Switch to new version", `engine-update.ts`) surfaces when `ready && !wasSwitchDismissed(disk_build_id)`, keyed on `version-status.disk_build_id`. `dismissToast` (store.ts) records the per-build dismissal but does NOT clear the badge signal.

The post-restart **"Engine restarted"** toast (`connection.ts`) is action-LESS — a pure engine-only Apply leaves the served client byte-identical, so nothing else surfaces. When a restart ALSO rebuilt the client (mixed change), the switched-to engine serves its newer pinned client, so `syncClientUpdateFromBuild` (re-run after the restart + via the SW nudges) sees the served `BUILD_ID` differ and surfaces the Refresh toast + dot together. The `ChangeApplied` arm only nudges the SW (`scheduleServiceWorkerUpdateChecks`) so the build-id check re-runs promptly once the rebuild lands. The dot also renders on the **reload icon in the workspace switcher** (control panel), mirroring the brand toggle's badge. Deliberately NOT part of the client check: the Tauri desktop app-version signal in `connection.ts` (a separate versioned-shell update mechanism) and the dev-only `import.meta.hot` HMR path in `main.tsx` (inert under built serving).

**Atomic dist publish (a failed rebuild can't 404 the app).** Vite empties the outDir at the start of every (re)build, so a failed/interrupted watch rebuild used to leave `dist/` with only the `public/` copy and no `index.html` — the static serve then 404s **every** route until the next *successful* rebuild (which only fires on the next source change), and with build output going to `/dev/null` the failure was invisible. `start_frontend_built` now runs the watch with `LUCIDOS_ATOMIC_DIST=1`, making the `lucidos-atomic-dist-publish` plugin (`crates/lucidos-app/vite.config.ts`) redirect `build.outDir` to `dist.staging/` and atomically rename it onto the live `dist/` in `closeBundle` — which Rollup runs only after a complete build. A crashed build never reaches `closeBundle`, so the last good `dist/` stays and the engine keeps serving it. Production builds (`npm run build` / CI / Tauri) run without the env var → `outDir` stays the default `dist/`, byte-identical to before. Build output goes to `crates/lucidos-app/.build-watch/log` (not `/dev/null`), and the launch's "Waiting for initial frontend build" line prints that path — so a build failure is one `tail` away instead of an unexplained 404.

**`public/` synced before the SW stamp (sw.js / manifest / favicons).** Each fresh `vite build` copies `publicDir` into the outDir, but the `lucidos-sync-public-dir` plugin (`crates/lucidos-app/vite.config.ts`) re-copies `public/` into the staging outDir on every `writeBundle`, ordered BEFORE `lucidos-sw-stamp`, so the re-copied `sw.js` is guaranteed present for its `BUILD_ID` stamp regardless of plugin ordering. (Historically this also covered the old `vite build --watch`, which copied `publicDir` only on the INITIAL build — vitejs/vite#18655 — so an incremental rebuild plus the atomic-dist swap WIPED `sw.js`/`manifest.json`/favicons from the served `dist/`, breaking SW registration and 404ing the PWA manifest. Fresh-build-per-change has no incremental rebuilds, but the plugin stays as the ordering backstop.) Production builds (no env var) copy `public/` correctly in one shot.

**Shared build-watch (checkout-level singleton).** `dist/` (plus `dist.staging`/`dist.prev`) is a SINGLE directory per checkout — every workspace launched from the same checkout serves the same `crates/lucidos-app/dist/`. So the build-watch is a checkout-level singleton, NOT per workspace: pid + log at `crates/lucidos-app/.build-watch/{pid,log}` (gitignored), tracked by `build_watch_pidfile`/`build_watch_log` in `scripts/lib/workspace.sh`. The first `--built` workspace to start it owns it; later launches **reuse** it.

- **Reuse rule** (`start_frontend_built`): if a healthy watch exists (live pid + `dist/index.html`) AND either another workspace is already serving this checkout (`running_frontend_workspaces_in_project` non-empty) OR this isn't an explicit `-b`, reuse without rebuilding; otherwise (re)build and take ownership — covering a dead watch and the **solo `-b`** rebuild.
- **Why:** the old `start_frontend_built` did `rm -rf dist` + a fresh build on EVERY startup — needless I/O on the shared tree. The determinism guard (`lucidos-sw-stamp` hashes asset names, so identical source → identical `BUILD_ID` → byte-identical `sw.js`) means a rebuild only changes the id when source actually differs. (This *used* to also toast every other workspace "New version available", because peers served the live `dist/` and their SW saw a new worker on EVERY rebuild including a no-op one. Impossible now — each engine serves its own pinned snapshot, and the peer sync only advances/toasts when the served `sw.js` BUILD_ID actually changed (`source_rebuilt`) AND advancing is INV-A-safe. The reuse rule stands purely as a build-I/O efficiency measure.)
- **Teardown is ref-counted:** `cleanup_processes` and `stop.sh` call `teardown_shared_build_watch_if_idle`, which kills the watch only when no workspace of the checkout is still serving the frontend (this workspace's `frontend.pid` is removed first, so it doesn't count itself).

**No stale-CSS wedge (fresh build per change).** The build-watch (`dev-build-watch.mjs`) runs a CLEAN `vite build` in a fresh child process on every change (`fs.watch` recursive over `src/`, `public/`, `index.html`, `vite.config.ts`, and the aliased SDK `src/`, debounced 200ms; a change mid-build is coalesced and rebuilt after). A fresh process has no long-lived Rollup incremental cache to corrupt, so the failure mode this section used to guard against — a days-old `vite build --watch` re-emitting fresh JS while serving a FROZEN CSS bundle (renamed/new classes unstyled, or a reverted color silently still showing), invisible to mtime/health checks — **can no longer happen**. The previous mitigations are therefore **removed**: the warn-only `cssStalenessGuard` plugin + `src/dev/cssWedgeDetect.ts`, and the 6h `BUILD_WATCH_MAX_AGE_S` age-recycle. Each build still stages into `dist.staging/` and publishes onto `dist/` only on success.

**Debugging a missed toast.** The System page's *Versions* section (Settings → System) shows both halves of the staleness comparison as build ids: **Client** = `CLIENT_BUILD_ID`, the build that produced the code executing right now (`virtual:build-id`), and **Service worker** = the active SW's `BUILD_ID` — the page asks the controlling SW via a `lucidos:get-build-id` message (SW replies `lucidos:build-id`), re-querying on `controllerchange` and each time the panel opens, so that one tracks the *live* worker. Both render as `dev` under the live dev server, where the stamp plugin is inert.

The **Client** row is deliberately NOT the engine's CalVer. It was, via a `virtual:engine-version` plugin, and the baked value froze at bundle-build time while the engine's VERSION kept bumping on every engine-only Apply — so the page showed two disagreeing version numbers that no reload could reconcile (`crates/lucidos-app/vite.config.ts` records why re-baking on a bump is worse than dropping it).

- Id unchanged across workspaces / across an apply ⇒ the SW never picked up a new build (rebuild or stamp issue).
- Id changed but no toast ⇒ suspect the toast logic — but only for a *freshly-served, never-dismissed* build. Badge and toast are coupled **on arrival** (the badge is `stale`; the toast surfaces alongside it when `stale && !dismissed`), so a badge without a toast is a regression **only on arrival**. A lone badge after a dismiss is the intended state (badge = affordance, toast deferred, remembered durably in `localStorage`).
- The served `BUILD_ID` is the running engine's **pinned** snapshot: a random mid-build shared-`dist` rebuild won't change it (invariant, not a bug). It advances on a Switch (respawn re-snapshots) **and** on a frontend-only Apply (`engine::frontend_refresh`, in-process re-snapshot, engine binary unchanged → compatible), so a frontend-only change surfaces badge/toast within a few seconds without a restart. A **mixed** change advances only via the Switch.

The old `--hmr` live-Vite-dev-server path was **removed** (ADR 0014): no Vite in the serving path to proxy to. The build-watch skips `tsc --noEmit` — type errors surface at the explicit build / in CC harden.

**Engine-restart interaction (the load-bearing part).** A CC Apply restarts the engine via `web-dev.sh --engine-only` (`crates/lucidos-engine/src/api/history.rs`), which sets `ENGINE_ONLY` and **exits before `start_vite`** — the restart never touches the frontend. `kill_stale_processes` skips the frontend-marker release when `ENGINE_ONLY` is set (and never touches the checkout-level shared build-watch on any per-workspace restart), so the running build-watch survives and the new engine re-serves the same `dist/`; the build-watch picks up the merged source and rebuilds on its own.

`build_sdk` still runs on this path (before the `ENGINE_ONLY` early-exit), so if the applied change bumped a dependency, `ensure_npm_deps` would want to reinstall the shared workspace `node_modules` — which it must NOT do under a live build-watch (corrupts Vite) and must NOT hard-fail over either (that abort left the workspace with no engine at all — the "workspace didn't come up after restart" bug). Under `ENGINE_ONLY` it **skips the install, warns, and returns 0**, so the engine comes up on the existing working `node_modules`; the stamp is left un-updated, so the deferred deps install on the next *full* restart (stop + `web-dev.sh`). The **`--engine-build` background rebuild** (`ENGINE_BUILD_ONLY`) gets the SAME treatment for the same reason: it too runs `build_sdk` while the frontend is live, and a hard-fail there aborts the whole background build — surfacing as a false "New engine version failed to build" even though the engine binary compiled fine (the on-disk binary is already written by then, so the next Apply also mis-fires "New version available" before its build finishes). Both `ENGINE_ONLY` and `ENGINE_BUILD_ONLY` are "keep the running frontend alive" paths, so `ensure_npm_deps` skips the install for either. (The `npm ci`-never-`npm install` rule that `ensure_npm_deps` implements is `.claude/rules/build-release.md` § "Lockfile determinism", ADR 0020.)

Implementation: `start_frontend_built` in `scripts/lib/workspace.sh` — checkout-level build-watch pid in `crates/lucidos-app/.build-watch/pid`, reused across workspaces; each workspace's `frontend.pid` records that shared pid as a ref-count marker; `release_frontend_marker` removes the file without killing the shared watch; `teardown_shared_build_watch_if_idle` (called by `cleanup_processes` and `stop.sh`) kills it only when no workspace of the checkout is left serving. The e2e harness (`scripts/lib/e2e.sh`) does NOT use the build-watch — one-shot `vite build`, legacy engine serves the resulting `dist/` via `LUCIDOS_STATIC_DIR`.

**e2e's one-shot build is staleness-aware, not existence-aware.** `ensure_frontend_built` (`scripts/lib/e2e.sh`) rebuilds when `dist/index.html` is **missing OR older than any build input**, and prints which branch it took (`Frontend build: REBUILDING …` / `REUSED …`) so an unattended nightly log states what the suite actually tested. The input set mirrors the watch list in `crates/lucidos-app/dev-build-watch.mjs` — `src/`, `public/`, `index.html`, `vite.config.ts`, `tsconfig.json`, `package.json`, the aliased workspace-local `packages/lucidos-sdk/src/`, and `crates/lucidos-engine/VERSION` (baked in by the engine-version plugin) — so a new build input must be added in BOTH places. It additionally covers the **repo-root `package.json` + `package-lock.json`**: npm workspaces hoist to the root and `npm ci` restores `node_modules` from the root lockfile, so a dependency bump changes the bundle without touching a single app file (the same two files `_deps_fingerprint` keys the install on). The check is one `find … -newer … -quit` (mtime, first hit wins), not a tree hash: git stamps checkout/merge time onto every file it writes, so moving frontend source forward always leaves it newer than a `dist/` built before it, and the failure direction is a redundant rebuild rather than a silently stale run. The old guard was `[ ! -f dist/index.html ]`, which ran the whole browser suite against a stale frontend and reported green whenever a checkout's `dist/` predated its frontend commits.

### Port allocation never signals an ancestor (ADR 0025)

`allocate_ports` can reclaim a port held by a **stale** `lucidos-engine`
(`_try_reclaim_stale_lucidos_on_port` → `kill -USR1`, the engine's real stop
signal — it ignores SIGTERM). The gate on that path is
`is_protected_host_pid`, and two of its four arms are **not defeatable by the
caller**: the pid must not be this process or an ancestor of it (cached `ps -o
ppid=` walk from `$$`), and `pid <= 1` is always refused. The other two arms —
`LUCIDOS_HOST_PID` / `LUCIDOS_FRONTEND_PID`, and a pidfile scan of
`<home>/workspaces/*/.lucidos/{engine,frontend}.pid` — stay, and that scan
covers the **password-database home** as well as `$HOME`.

Two rules follow, and both are load-bearing:

- **Sandboxing `HOME` does NOT disarm host protection**, by design. It still
  isolates the port *registry* (`LUCIDOS_PORT_REGISTRY` is `$HOME`-relative).
  A test that needs a pid to be *unprotected* must use a dead or synthetic one
  — the `kill -0` liveness gate on the pidfile arms is what makes that work.
- **A `scripts/lib/*_test.sh` that sources `ports.sh` must stub `lsof` for the
  WHOLE file**, not per test. Stubbing `port_is_free` alone is not enough:
  `_port_is_ours_or_free` calls `lsof -ti :<port> -sTCP:LISTEN` separately, and
  an unstubbed call resolves a REAL host pid. `ports_test.sh` does this, plus a
  `kill` shim that refuses any lethal signal to a pid it didn't spawn and fails
  the suite if one was attempted — copy that pattern rather than inventing
  another. This is not hypothetical: it is why the machine's dev engine died
  twice on 2026-07-28.

## Engine tests need Postgres — use `test-engine.sh`

The engine's integration tests (`setup_test_db` in `crates/lucidos-engine/src/test_support.rs`) need a **real Postgres**: each test `CREATE`s a throwaway `lucidos_test_*` database, runs migrations, drops it. The connection comes from `TEST_DATABASE_URL`, falling back to a hardcoded `localhost:5432`. **Bare `cargo test -p lucidos-engine` with no `TEST_DATABASE_URL` and no PG up makes every DB-backed test panic on connect** (`.expect("admin connect")`) — hundreds of false "failures", not regressions.

```bash
make test                       # → ./scripts/test-engine.sh  (cargo test --lib)
make test-full                  # → ./scripts/test-engine.sh --full  (whole crate)
./scripts/test-engine.sh -- -- migration_tests   # pass filters through to cargo test
./scripts/test-engine.sh --fresh                 # recreate the test DB container clean
```

`test-engine.sh` provisions a **dedicated, disposable** `lucidos-pg-test` container (`pgvector/pgvector:pg18`, port `LUCIDOS_TEST_PG_PORT` / default `5510`), exports `TEST_DATABASE_URL`, then runs cargo test. Isolated from every workspace's PG (separate name + port) so a test run can't mutate `~/workspaces/*` data, and it **never broad-kills** — it touches only its own container by exact name (the prior `test-engine.sh` was deleted for `pkill -f cognos-engine`). To run cargo directly instead, start the container once and `export TEST_DATABASE_URL` yourself.

Always use `web-dev.sh -b` to restart. `scripts/lib/ports.sh` allocates per-workspace engine ports; the engine serves the built `dist/` directly (`LUCIDOS_STATIC_DIR`, ADR 0014 — no Vite proxy). The shared Postgres container stays running when one workspace stops; legacy `lucidos-pg-<cksum>` containers stay intact only as rollback sources until decommissioned.

**e2e runs on a RELEASE engine by default.** `scripts/lib/e2e.sh` (sourced by `e2e.sh` / `e2e-browser.sh` / `e2e-api.sh`) sets `RELEASE=1` so `build_or_find_engine` builds + serves `target/release/launch/e2e-test-hooks/lucidos-engine` (ADR 0022 — its own variant dir, disjoint from a dev workspace's) — the debug engine's CPU cost drove the mobile-webkit contention wedge, and release matches the packaged/prod engine. `LUCIDOS_E2E_DEBUG=1` opts back to the fast debug build for local iteration; `CARGO_BUILD_JOBS` is capped at half the cores on the release path to avoid a codegen OOM. See `.claude/rules/testing.md` and `docs/plans/2026-06-28-e2e-always-release-build.md`.

### macOS code signing (stable TCC grants)

A `cargo build` engine binary is `adhoc, linker-signed`; its CDHash changes every rebuild, so macOS TCC discards prior permission grants and re-prompts ("lucidos-engine would like to access …") after each rebuild. `build_or_find_engine` (`scripts/lib/workspace.sh`) re-signs the freshly built binary with a **stable self-signed identity** (`scripts/lib/codesign.sh` → `sign_engine_binary`), giving it a rebuild-stable Designated Requirement so one Allow click persists. Run `./scripts/dev-codesign-setup.sh` **once** first — it creates + trusts the cert (single GUI password prompt). Until then signing is a no-op and the build proceeds unsigned (with a hint). This only stops the re-prompting; the prompt still names "lucidos-engine" (a post-fork TCC responsibility disclaim to attribute it to Claude Code is not possible — see the note in `runtime/claude_code.rs::build_command`).

**Search-list registration is load-bearing.** `codesign --sign <name>` resolves the identity through the **keychain search list**, not the `--keychain` flag — so the dedicated `lucidos-dev-signing.keychain-db` must be *in the search list* or every sign fails with "no identity found" and silently falls back to ad-hoc (prompts never stop, even though `find-identity -p codesigning "$KEYCHAIN"` reports the identity as valid). `lucidos_ensure_keychain_in_search_list` (in `codesign.sh`) registers it; both setup and `sign_engine_binary` call it, so existing installs self-heal on the next `-b` build. **Per-binary, not per-workspace:** every engine binary signed with the same identifier (`lucidos-engine`) + same cert leaf shares one Designated Requirement, so a single Allow covers all workspaces. A binary built outside the scripts — e.g. `cargo run` from an IDE — bypasses `sign_engine_binary` and stays ad-hoc; launch via `web-dev.sh` so it gets signed.
