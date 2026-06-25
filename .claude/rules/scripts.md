---
globs:
  - "scripts/**"
  - "**/*.sh"
  - "Makefile"
---

# Scripts & Build

## Dev / runtime scripts

```bash
./scripts/web-dev.sh -w <ws> [-b] [-r]    # Start (-b builds engine+gateway; -r release; engine serves built dist/)
./scripts/tauri-dev.sh -w <ws> [-b]       # Start engine + Tauri window
./scripts/stop.sh -w <ws>                 # Stop a specific workspace
./scripts/status.sh                       # Check running status
./scripts/populate.sh -w <ws> [-c]        # Populate test history
./scripts/new-migration.sh <description>  # Create timestamped migration
./scripts/dev-codesign-setup.sh           # One-time: stable macOS code-signing identity
./scripts/test-engine.sh [--full|--fresh] # Engine tests against a dedicated Docker PG
./scripts/e2e-packaged.sh [--rebuild]     # macOS-only: boot the packaged .app (service + embedded PG) and smoke-test the chain (heavy: builds the .app)
```

### Workspace gateway + dev topology (ADR 0014 — Dev ≠ packaged!)

**Dev runtime topology is two ports, both live at once** (ADR 0014 §4 + the
normative "Dev runtime topology" table — read it before touching ports/binds):

| | binds | URL | serves |
|---|---|---|---|
| **engine** (one per workspace) | `ENGINE_PORT` = `VITE_PORT` (5173+offset), **all interfaces** | `https://localhost:5173/` | the workspace app at `/` (base `/`) — directly, exactly as the pre-gateway engine did |
| **gateway** (ONE per machine) | `GATEWAY_PORT` = **fixed 5251** (override `LUCIDOS_DEV_GATEWAY_PORT`) | `https://localhost:5251/<slug>/` + picker `…/~/` | proxies `/<slug>/` to each engine; serves the picker listing **every** launched workspace. Dev uses **5251**, NOT 5252 — 5252 is the packaged `Lucidos.app` gateway, so dev + packaged coexist out of the box |

**Loopback-only is the PACKAGED posture, NOT dev.** In dev the engine binds all
interfaces on its own port so `https://localhost:5173/` reaches it directly; the
gateway (`LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0`) spawns it that way and proxies +
health-probes it over **https** (self-signed cert accepted). Do not make the dev
engine loopback-only — that breaks direct access and contradicts §4.

`web-dev.sh -w <ws>` (`scripts/lib/workspace.sh`): `swap_ports` sets
`ENGINE_PORT=VITE_PORT` (per-workspace) + `GATEWAY_PORT=5251` (**fixed, shared** —
override `LUCIDOS_DEV_GATEWAY_PORT`; 5251 keeps dev clear of the packaged app's
5252); `build_or_find_engine` builds + signs BOTH
`lucidos-engine` and `lucidos-gateway`; `seed_gateway_registry` upserts this
workspace into the **machine-global** registry
`$HOME/.lucidos/gateway/config/workspaces.json` (NOT per-workspace) — refreshing
its **direct** engine port and workspace directory, removing any legacy
`database_url`, and **preserving** any picker-set display name + `autostart`
flag; a brand-new entry defaults `autostart:false`. Postgres is one shared
Docker container/volume (`lucidos-pg-shared` / `lucidos-pg-data-shared`) with one
database per workspace (`lucidos_<slug>`); legacy per-workspace containers are
migration sources only until explicitly decommissioned. `start_gateway` then runs
ONE shared gateway: it reuses a healthy one already on 5251, else starts it under
the **dedicated gateway supervisor** `run_gateway_supervised`
(`scripts/lib/gateway_supervisor.sh` — NOT the engine's `run_supervised`; the
gateway is a machine-global daemon, so its supervisor `trap '' SIGHUP SIGINT
SIGTERM` and is launched `disown`ed so it survives the launching `web-dev.sh`
shell + terminal close, the way the packaged Rust `--service` survives under
launchd `KeepAlive`. Its only legitimate stop is SIGUSR1 to the gateway child)
(`LUCIDOS_ENGINE_BIN=<engine>`, `LUCIDOS_STATIC_DIR=<dist>`,
`LUCIDOS_API_PORT=5251`, `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0`,
`LUCIDOS_GATEWAY_DATA=$HOME/.lucidos/gateway`,
`LUCIDOS_GATEWAY_PG_BACKEND=docker`, `LUCIDOS_GATEWAY_PG_PORT=<shared-pg-port>`,
`LUCIDOS_GATEWAY_PG_CONTAINER=lucidos-pg-shared`) — either way it then POSTs
`/~/api/v1/control/workspaces/<id>/restart` to start (or respawn, for Apply) THIS
workspace's engine, since new workspaces default `autostart:false` so the
gateway's own boot won't spawn them. The gateway reverse-proxies `/<slug>/` as a
pure streaming forward — strips `/<slug>`, adds `X-Forwarded-Prefix: /<slug>/`,
forwards the response untouched (no body rewrite). Gateway-owned surface (picker,
control API, health) lives behind the reserved **sigil namespace `/~/`**; `/`
smart-redirects into the sole workspace, or serves the picker when there are
several. The picker lists **every workspace ever launched** (durable membership);
a stopped one stays listed and **lazy-starts** on a proxy hit / explicit open.
**Probing a served asset through the gateway needs the workspace prefix.** To
curl a served file (SDK bundle, static asset, an API route) you MUST hit
`https://<host>/<slug>/api/v1/...`, NOT a bare `https://<host>/api/v1/...` — the
gateway resolves the FIRST path segment as a workspace slug, so a bare
`/api/v1/...` is read as workspace `api` and 404s with `unknown workspace 'api'`.
(Hitting an engine directly on its own port is base `/`, so `…:<port>/api/v1/…`
works there — the prefix gotcha is gateway-only.)
More workspaces are also created from the picker (the gateway provisions their
Docker Postgres itself, container `lucidos-pg-gw-<id>`). `stop.sh -w <ws>` does
**NOT** kill the shared gateway — it POSTs
`/~/api/v1/control/workspaces/<id>/stop` (the gateway drops that stack so its
supervisor won't respawn it; the registry entry survives, so the workspace stays
listed) and leaves the gateway up for peers. Stop the gateway itself with
`kill $(cat $HOME/.lucidos/gateway/gateway.pid)`.

- **Standalone crate (ADR 0014 §1):** the gateway is `crates/lucidos-gateway/`
  with NO dependency on `lucidos-engine` — the only network-facing process links
  proxy + supervise + registry code, not the engine's heavy core. It spawns the
  engine by path via `LUCIDOS_ENGINE_BIN` (its own `current_exe` is the gateway).
- **Engine serves the frontend directly:** both the gateway (picker) and every
  spawned engine serve the built `dist/` from `LUCIDOS_STATIC_DIR`. The engine
  stamps `<base href="/<slug>/">` into `index.html` from `X-Forwarded-Prefix`
  (default `/` when hit directly) so relative asset refs resolve back through the
  gateway. There is **no Vite in the serving path** (no `dev_proxy`, no `vite
  preview`).
- **Apply restart** (`--engine-only`): in gateway dev, `kill_stale_processes`
  leaves the gateway alive and `start_gateway` reuses it + POSTs
  `/~/api/v1/control/workspaces/<id>/restart`, so only the active workspace's
  engine respawns onto the rebuilt binary (peers untouched). A full `-b` instead
  stops the gateway (it re-adopts running peers) so the rebuilt gateway binary is
  used. The engine's `/api/v1/restart` handler spawns `web-dev.sh --engine-only`
  in dev so the rebuild happens (packaged POSTs the gateway control API directly).
- **Gateway self-reload (picker reload control):** because the `--engine-only`
  Apply restart leaves the shared gateway running its already-compiled binary, a
  change to `crates/lucidos-gateway/**` (e.g. the boot-splash HTML) is rebuilt on
  disk but NOT served until the gateway itself restarts. The workspace picker
  surfaces this: `GET /~/api/v1/control/gateway/status` returns
  `{build_id, update_available, packaged}` — the running process's baked
  `GATEWAY_BUILD_ID` (git short SHA + a hash of any uncommitted gateway-source
  diff, baked by `crates/lucidos-gateway/build.rs`; printable via
  `lucidos-gateway --build-id`) and whether the on-disk binary's id differs
  (checked behind a cheap `current_exe` mtime gate so the picker's 2s poll doesn't
  fork per tick). The picker shows a reload icon, badged when `update_available`.
  `POST /~/api/v1/control/gateway/reload` makes the gateway **re-exec itself** onto
  the on-disk binary (`execv(current_exe, argv)`): SAME PID, so the supervisor
  keeps `wait`ing on it (no respawn) and `gateway.pid` stays valid; the fresh
  `main()` re-adopts the running engines on boot. This is the ONLY in-place gateway
  restart — note it is distinct from the supervisor's SIGUSR1, which is the
  gateway's *permanent* stop (clean exit → supervisor stops, see
  `scripts/lib/gateway_supervisor.sh`), not a restart. The endpoint returns 202
  before the (short-delayed) exec so the picker's request still resolves.
  **The self-reload control is DEV-ONLY:** the status `packaged` field is `true`
  under the packaged desktop runtime (`desktop.rs::spawn_gateway` sets
  `LUCIDOS_PACKAGED=1`; dev's `web-dev.sh` sets nothing → `false`), and the picker
  renders the reload icon only when `!packaged`. A re-exec onto a rebuilt on-disk
  binary only makes sense in dev (a CC Apply rebuilds the gateway binary under a
  running gateway); a packaged build never rebuilds in place — its updates go
  through the app updater + a full launchd service restart (see
  `crates/lucidos-app/src/updater.rs` and `docs/desktop-app.md`).
- **Auto-start + boot (ADR 0014):** the registry's per-workspace `autostart` flag
  (picker toggle → `POST /~/api/v1/control/workspaces/<id>/autostart {enabled}`)
  governs gateway boot — it **re-adopts** already-running engines, **spawns** the
  auto-start workspaces, and leaves the rest **stopped** (lazy-started on open).
  New dev workspaces default `autostart:false`: an explicit `web-dev.sh` launch
  starts them for the session (via the restart POST above) but they won't
  auto-start on a future gateway boot until toggled on. (There is no auto-created
  `default`: on a truly empty registry the gateway creates nothing and the smart
  root serves the picker.) This is why the dev launcher always POSTs restart for
  the launched workspace rather than relying on the gateway's boot.
- **Shared Postgres (ADR 0014 §6/§7):** the dev launcher starts/verifies one
  shared Docker Postgres cluster and ensures the launched workspace database
  exists. If a legacy per-workspace `lucidos-pg-<cksum>` cluster exists and the
  shared database is not verified, the launcher dumps/restores the old `lucidos`
  database into `lucidos_<slug>`, verifies the target, writes a marker under
  `.lucidos/`, and leaves the old container/volume intact. Remove legacy data
  only with `./scripts/decommission-legacy-postgres.sh -w <ws>`, which refuses
  without the marker and a reachable shared database.
- **Escape hatch:** `LUCIDOS_NO_GATEWAY=1 ./scripts/web-dev.sh -w <ws>` runs the
  legacy single-engine model (no gateway at all); the engine serves the app at
  `/` with base `/`. (This is a *separate* mode from the dev engine's direct
  access above, which coexists with the gateway.)
- **e2e** drives the legacy direct-engine model (`scripts/lib/e2e.sh` calls
  `start_engine` directly with `LUCIDOS_STATIC_DIR` set + a one-shot `vite
  build`); the frontend is served at `/` (base path `''`).

### Frontend: the engine serves the built `dist/` directly (ADR 0014)

`web-dev.sh` serves a **built frontend**: `start_frontend_built` runs the
build-watch (`crates/lucidos-app/dev-build-watch.mjs`), which does an initial
`vite build` then a **fresh `vite build` (clean child process) on every source
change** — producing the bundled `dist/`, which the **engine serves directly**
via `LUCIDOS_STATIC_DIR`
(there is no `vite preview` and no engine→Vite proxy). Content-hashed `/assets/*`
are cached **cache-first** by
the service worker (immutable by hash), so a reload pulls the heavy JS/CSS graph
from disk; the navigation shell (`index.html`) is served **network-first**
(`networkFirstShell` in `sw.js`), falling back to the cached shell only when
offline → fast iOS PWA resume / notification-tap reload without the ~10s cold-load
black screen over Tailscale, while keeping the shell in lockstep with the assets
the rebuilt server currently has. (The shell used to be cache-first too, but a
long-lived PWA then pinned a stale `index.html` referencing `/assets/*` bundles a
later `vite build --watch` had deleted; the SPA fallback served those as
`text/html`, the entry module failed to parse, and the PWA went all-black — see
`system-knowhow/notifications.md` §4.5 thirteenth iteration.) The client only
changes when rebuilt, so it can't drift ahead of the engine binary. Trade-off: **no HMR** — after a change is applied, the build-watch runs a fresh
`vite build` (sub-second here), the engine serves the new `dist/`, the SW detects the new build (each build stamps a fresh
`BUILD_ID` into `sw.js` — and the same id into the app bundle as `CLIENT_BUILD_ID`
via the `virtual:build-id` module — through the `lucidos-sw-stamp` plugin in
`crates/lucidos-app/vite.config.ts`), and the existing **"New version available →
Refresh"** toast tells you when to reload. The "client update available" dot is
driven by comparing the running bundle's `CLIENT_BUILD_ID` against the served
`sw.js` `BUILD_ID` (`syncClientUpdateFromBuild`) — an honest "is my loaded code
stale?" signal that self-clears once a reload lands on the served build, rather
than latching on the controlling worker's id.

**Atomic dist publish (a failed rebuild can't 404 the app).** Vite empties the
outDir at the start of every (re)build, so a watch rebuild that fails or is
interrupted used to leave the served `dist/` with only the `public/` copy and no
`index.html` — the engine's static serve then 404s **every** route until the next
*successful* rebuild (which only fires on the next source change), and because the
build output went to `/dev/null` the failure was invisible. Now
`start_frontend_built` runs the watch with `LUCIDOS_ATOMIC_DIST=1`, which makes
the `lucidos-atomic-dist-publish` plugin (`crates/lucidos-app/vite.config.ts`)
redirect `build.outDir` to `dist.staging/` and atomically rename it onto the live
`dist/` in `closeBundle` — which Rollup runs *only after a complete build*. A
crashed build never reaches `closeBundle`, so the last good `dist/` stays in place
and the engine keeps serving it. Production builds (`npm run build` / CI / Tauri)
run without the env var → `outDir` stays the default `dist/`, byte-identical to
before. Build output goes to `crates/lucidos-app/.build-watch/log` (not
`/dev/null`), and the launch's "Waiting for initial frontend build" line prints
that path — so a build failure is one `tail` away instead of an unexplained 404.

**`public/` synced before the SW stamp (sw.js / manifest / favicons).** Each
fresh `vite build` copies `publicDir` into the outDir as part of the build, but
the `lucidos-sync-public-dir` plugin (`crates/lucidos-app/vite.config.ts`)
re-copies `public/` into the staging outDir on every `writeBundle`, ordered
BEFORE `lucidos-sw-stamp`, so the re-copied `sw.js` is guaranteed present for its
`BUILD_ID` stamp regardless of plugin ordering. (Historically this also covered
the old `vite build --watch`, which copied `publicDir` only on the INITIAL build
— vitejs/vite#18655 — so an incremental rebuild combined with the atomic-dist
swap WIPED `sw.js`/`manifest.json`/favicons from the served `dist/`, breaking SW
registration and 404ing the PWA manifest. Fresh-build-per-change no longer has
incremental rebuilds, but the plugin stays as the ordering backstop.) Production
builds (no env var) copy `public/` correctly in one shot, so they're untouched.

**Shared build-watch (checkout-level singleton).** `dist/` (plus `dist.staging`/
`dist.prev`) is a SINGLE directory per checkout — every workspace launched from
the same checkout serves the same `crates/lucidos-app/dist/` (each engine serves
it directly via `LUCIDOS_STATIC_DIR`). So the build-watch that produces it is a
checkout-level singleton, NOT per workspace: its pid + log live at
`crates/lucidos-app/.build-watch/{pid,log}` (gitignored), tracked by
`build_watch_pidfile`/`build_watch_log` in `scripts/lib/workspace.sh`. The first
`--built` workspace to start it owns it; later launches **reuse** it. The reuse
rule lives in `start_frontend_built`: if a healthy watch exists (live pid +
`dist/index.html`) AND either another workspace is already serving this checkout
(`running_frontend_workspaces_in_project` non-empty) OR this isn't an explicit
`-b`, it reuses without rebuilding; otherwise it (re)builds and takes ownership —
which covers a dead watch and the **solo `-b`** rebuild. This is the fix
for "starting a new workspace toasted every other workspace 'New version
available'": the determinism guard (vite.config.ts `lucidos-sw-stamp` hashes
asset names, so identical source → identical `BUILD_ID` → byte-identical `sw.js`)
means a rebuild only changes the id when source actually differs — but the old
`start_frontend_built` did `rm -rf dist` + a fresh build on EVERY startup, which
republished the shared `dist/` and dragged other workspaces' open tabs forward
(their SW saw a new worker). Reusing instead of republishing leaves their served
`sw.js` untouched. Teardown is **ref-counted**: `cleanup_processes` and `stop.sh`
call `teardown_shared_build_watch_if_idle`, which kills the watch only when no
workspace of the checkout is still serving the frontend (this workspace's
`frontend.pid` is removed first, so it doesn't count itself). Caveat of sharing:
a rebuild republishes the shared `dist/`/`sw.js` and so fires "New version
available" on peers' open tabs — which is exactly why the reuse rule above avoids
rebuilding when a peer is already serving.

**No stale-CSS wedge (fresh build per change).** The build-watch
(`dev-build-watch.mjs`) runs a CLEAN `vite build` in a fresh child process on
every change (`fs.watch` recursive over `src/`, `public/`, `index.html`,
`vite.config.ts`, and the aliased SDK `src/`, debounced 200ms; a change mid-build
is coalesced and rebuilt after). A fresh process has no long-lived Rollup
incremental cache to corrupt, so the failure mode this whole section used to
guard against — a days-old `vite build --watch` re-emitting fresh JS while serving
a FROZEN CSS bundle (renamed/new classes unstyled, or a reverted color silently
still showing), invisible to mtime/health checks — **can no longer happen**: each
rebuild re-reads all source from disk, so the served `dist/` always reflects
current source. The previous mitigations are therefore **removed**: the warn-only
`cssStalenessGuard` plugin + `src/dev/cssWedgeDetect.ts`, and the 6h
`BUILD_WATCH_MAX_AGE_S` age-recycle. Builds are sub-second, so a full build per
change costs nothing noticeable. (Each build still stages into `dist.staging/` and
atomically publishes onto `dist/` only on success — a failed build never clobbers
the served `dist/`.)

**Debugging a missed toast:** the connection-status popover (control panel) shows
the active SW's `BUILD_ID` as a **Build** row. The page asks the controlling SW for
it via a `lucidos:get-build-id` message (SW replies `lucidos:build-id`), re-querying
on `controllerchange` and each time the panel opens, so the shown id tracks the
*live* worker. If the id is unchanged across workspaces / across an apply, the SW
never picked up a new build (rebuild or stamp issue); if it changed but no toast
fired, the toast logic is the suspect.

The old `--hmr` live-Vite-dev-server path was **removed** (ADR 0014): there is no
Vite in the serving path to proxy to, so the engine serves the built `dist/`
everywhere. The build-watch skips `tsc --noEmit` — type errors surface at the
explicit build / in CC harden.

**Engine-restart interaction (the load-bearing part):** a CC Apply restarts the
engine via `web-dev.sh --engine-only` (`crates/lucidos-engine/src/api/history.rs`),
which sets `ENGINE_ONLY` and **exits before `start_vite`** — so the restart never
touches the frontend. `kill_stale_processes` skips the frontend-marker release
when `ENGINE_ONLY` is set (and never touches the checkout-level shared build-watch
on any per-workspace restart), so the running build-watch survives the restart and
the new engine just re-serves the same `dist/` (`LUCIDOS_STATIC_DIR`); the
build-watch picks up the merged source and rebuilds `dist/` on its own.
`build_sdk` still runs on this path (before the `ENGINE_ONLY` early-exit), so if
the applied change bumped a dependency, `ensure_npm_deps` would want to reinstall
the shared workspace `node_modules` — which it must NOT do under a live build-watch
(corrupts Vite) and must NOT hard-fail over either (that abort left the workspace
with no engine at all — the "workspace didn't come up after restart" bug). Under
`ENGINE_ONLY` it instead **skips the install, warns, and returns 0** so the engine
comes up on the existing (working) `node_modules`; the stamp is left un-updated, so
the deferred deps install on the next *full* restart (stop + `web-dev.sh`).
Implementation: `start_frontend_built` in `scripts/lib/workspace.sh`
(checkout-level build-watch pid in `crates/lucidos-app/.build-watch/pid`, reused
across workspaces; each workspace's `frontend.pid` records that shared build-watch
pid as a ref-count marker — `release_frontend_marker` removes the file without
killing the shared watch, and `teardown_shared_build_watch_if_idle` (called by
`cleanup_processes` and `stop.sh`) kills it only when no workspace of the checkout
is left serving). The e2e harness (`scripts/lib/e2e.sh`) does NOT use the
build-watch — it does a one-shot `vite build` and the legacy engine serves the
resulting `dist/` directly via `LUCIDOS_STATIC_DIR`.

## Engine tests need Postgres — use `test-engine.sh`

The engine's integration tests (`setup_test_db` in `crates/lucidos-engine/src/test_support.rs`) need a **real Postgres**: each test `CREATE`s a throwaway `lucidos_test_*` database, runs migrations, and drops it. The connection comes from `TEST_DATABASE_URL`, falling back to a hardcoded `localhost:5432`. **Running bare `cargo test -p lucidos-engine` with no `TEST_DATABASE_URL` and no PG up makes every DB-backed test panic on connect** (`.expect("admin connect")`) — that's hundreds of false "failures", not regressions.

```bash
make test                       # → ./scripts/test-engine.sh  (cargo test --lib)
make test-full                  # → ./scripts/test-engine.sh --full  (whole crate)
./scripts/test-engine.sh -- -- migration_tests   # pass filters through to cargo test
./scripts/test-engine.sh --fresh                 # recreate the test DB container clean
```

`test-engine.sh` provisions a **dedicated, disposable** `lucidos-pg-test` container (`pgvector/pgvector:pg18`, port `LUCIDOS_TEST_PG_PORT` / default `5510`), exports `TEST_DATABASE_URL`, then runs cargo test. It is isolated from every workspace's PG (separate name + port) so a test run can't mutate `~/workspaces/*` data, and it **never broad-kills** — it touches only its own container by exact name (the prior `test-engine.sh` was deleted for `pkill -f cognos-engine`). To run cargo directly instead, start the container once and `export TEST_DATABASE_URL` yourself.

Always use `web-dev.sh -b` to restart. `scripts/lib/ports.sh` allocates per-workspace engine ports; the engine serves the built `dist/` directly (`LUCIDOS_STATIC_DIR`, ADR 0014 — no Vite proxy). The shared Postgres container stays running when one workspace stops; legacy `lucidos-pg-<cksum>` containers stay intact only as rollback sources until decommissioned.

### macOS code signing (stable TCC grants)

A `cargo build` engine binary is `adhoc, linker-signed`; its CDHash changes every rebuild, so macOS TCC (privacy) discards prior permission grants and re-prompts ("lucidos-engine would like to access …") after each rebuild. `build_or_find_engine` (in `scripts/lib/workspace.sh`) re-signs the freshly built binary with a **stable self-signed identity** (`scripts/lib/codesign.sh` → `sign_engine_binary`), giving it a rebuild-stable Designated Requirement so one Allow click persists. Run `./scripts/dev-codesign-setup.sh` **once** first — it creates + trusts the cert (single GUI password prompt). Until then signing is a no-op and the build proceeds unsigned (with a hint). This only stops the re-prompting; the prompt still names "lucidos-engine" (a post-fork TCC responsibility disclaim to attribute it to Claude Code is not possible — see the note in `runtime/claude_code.rs::build_command`).

**Search-list registration is load-bearing.** `codesign --sign <name>` resolves the identity through the **keychain search list**, not the `--keychain` flag — so the dedicated `lucidos-dev-signing.keychain-db` must be *in the search list* or every sign fails with "no identity found" and silently falls back to ad-hoc (the prompts never stop, even though `find-identity -p codesigning "$KEYCHAIN"` reports the identity as valid). `lucidos_ensure_keychain_in_search_list` (in `codesign.sh`) registers it; both setup and `sign_engine_binary` call it, so existing installs self-heal on the next `-b` build. **Per-binary, not per-workspace:** every engine binary signed with the same identifier (`lucidos-engine`) + same cert leaf shares one Designated Requirement, so a single Allow covers all workspaces. But a binary built outside the scripts — e.g. `cargo run` from an IDE — bypasses `sign_engine_binary` and stays ad-hoc; launch via `web-dev.sh` so it gets signed.

## Build

```bash
cargo build -p lucidos-engine --release    # Engine
cd crates/lucidos-app && cargo tauri build # Desktop app
```

Dev: native engine + Docker PostgreSQL. Production: single Docker container. Makefile: `make build`, `make test`, `make run`.
