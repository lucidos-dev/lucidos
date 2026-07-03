---
globs:
  - "scripts/**"
  - "**/*.sh"
  - "Makefile"
---

# Scripts & Build

## Dev / runtime scripts

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
engine loopback-only — that breaks direct access and contradicts §4. **The
gateway ITSELF also binds all interfaces in dev** via `LUCIDOS_GATEWAY_BIND_ALL=1`
(set by `start_gateway`, the sibling opt-in to `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0`):
the gateway defaults to loopback-only as its packaged security posture, so dev —
which fronts the picker + `/<slug>/` routing for other devices (e.g. an iOS PWA
over Tailscale) — must opt in explicitly, or a gateway rebuild+reload comes back
up on `127.0.0.1` only and is unreachable remotely. Packaged
(`desktop.rs::spawn_gateway`, `LUCIDOS_PACKAGED=1`) does NOT run `start_gateway`,
so it stays loopback-only.

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
`LUCIDOS_GATEWAY_BIND_ALL=1`,
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
- **Apply → background build, then switch** (new-version/switch flow, ADR/plan
  `docs/plans/2026-07-01-new-engine-version-switch-flow.md`): *Apply* is
  non-disruptive. For an engine-affecting change (dev), the engine kicks off a
  BACKGROUND rebuild via `web-dev.sh --engine-build` (build-only: `build_or_find_engine`
  + `build_sdk`, NO kill/respawn/Vite) while the running engine keeps serving; a
  second Apply coalesces (aborts + restarts the build). When the on-disk binary's
  `ENGINE_BUILD_ID` differs from the running one, the frontend surfaces
  "New version available → Switch to new version" (`GET /api/v1/engine/version-status`
  poll). The **switch** (`/api/v1/restart`) only RESPAWNS onto the already-built
  binary — no build at switch: in gateway dev/packaged it POSTs
  `/~/api/v1/control/workspaces/<id>/restart` (gateway SIGUSR1s + respawns the
  engine, peers untouched); legacy `LUCIDOS_NO_GATEWAY` dev falls back to
  `web-dev.sh --engine-only` (a fast near-noop build then respawn); packaged
  without a gateway uses launchd. Boundary "Switched to new version" events are
  emitted at ACTUAL teardown by `main.rs::shutdown_signal`, never during the build.
  A full `-b` still stops the gateway so a rebuilt gateway binary is used.
- **Consistent version signal + self-heal (`docs/plans/2026-07-03-engine-version-switch-selfheal.md`).**
  The "New version available" surface used to be driven ONLY by
  `version-status.update_available` (`on-disk binary build-id ≠ running`). That is a
  dead-end when the background rebuild that would advance the shared binary **fails
  or never completes** (e.g. the concurrent-`target/`-build failure below): the
  binary stays stale ⇒ `update_available` stays false ⇒ NO Switch — while the
  frontend-only-Apply INV-A veto (`engine_source_matches_head`) simultaneously and
  correctly defers every frontend-only Apply to that never-arriving Switch. All
  co-located workspaces then serve stale JS with no actionable UI. Two fixes:
  - **Consistent signal.** `version-status` also reports **`source_behind_head`** —
    the engine SOURCE is behind HEAD by a restart-requiring change (the SAME
    `engine_source_matches_head` git classifier the veto uses), so a NEW engine
    version is discoverable even before a fresh binary exists. TTL-cached
    (`engine_version::source_behind_head`, `SOURCE_BEHIND_TTL`) so the `git diff`
    runs at most once per interval regardless of client count. The frontend surfaces
    it as a "New engine version pending — Rebuild" toast (`checkEngineVersion`,
    gated on `!update_available` so it never nags once a switchable binary exists),
    plus a "Retry build" action on the build-failed toast; `POST /api/v1/engine/rebuild`
    is the manual trigger behind both (no-op packaged).
  - **Self-heal.** The dev periodic loop (`frontend_refresh::spawn_served_frontend_sync`,
    ~10s) runs `self_heal_engine_version_if_needed`: when source is behind with a
    stale on-disk binary and no build in flight, it (re)triggers a background
    rebuild so the Switch surfaces WITHOUT a manual `-b`. **Bounded** —
    `SELF_HEAL_MAX_ATTEMPTS_PER_HEAD` per HEAD (reset when HEAD moves) so a
    genuinely broken `main` can't spin builds forever; the build-failed toast stays
    surfaced. **Coordinated** — co-located workspaces share ONE checkout + ONE
    `target/`, so `run_engine_build` holds a checkout-shared advisory **build lock**
    (`fs2` flock at `<repo_root>/target/.lucidos-engine-build.lock`, auto-released
    on drop / process death): exactly one `web-dev.sh --engine-build` runs at a
    time; the others get `EngineBuildOutcome::SkippedLocked` (→ `build_state` back
    to `Idle`, NOT `Failed`) and observe the shared binary advance. This upholds the
    "never two concurrent cargo builds on the same target" rule (CLAUDE.md) — the
    concurrent-rebuild collision was the likely original cause of the failure that
    wedged everything. (Scope: the lock serializes the *engine-triggered* builds —
    Apply, self-heal, `POST /engine/rebuild` — which are what fire automatically en
    masse across workspaces. A human `web-dev.sh -b` is a deliberate single action
    and is NOT lock-coordinated — macOS ships no `flock` binary, so shell-side
    locking is out of scope here.)
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
`vite build` (sub-second here) into the shared `dist/`, stamping a fresh `BUILD_ID`
into `sw.js` (and the same id into the app bundle as `CLIENT_BUILD_ID` via the
`virtual:build-id` module, through the `lucidos-sw-stamp` plugin in
`crates/lucidos-app/vite.config.ts`).

**The running engine serves ONLY a client compatible with itself — never a newer
one, not even on reload.** In dev the engine does NOT serve the live `dist/`: at
boot it takes a private **pinned snapshot** of `dist/`
(`<workspace>/.lucidos/served-frontend/<generation>/`, hardlink-copy — a numbered
subdir per snapshot; `crates/lucidos-engine/src/api/frontend_snapshot.rs`) behind
a **swappable handle** (`Arc<RwLock<PathBuf>>`) and serves THAT (`serve_frontend`
reads the current generation per request). The build-watch keeps advancing the
shared `dist/`, but the running engine keeps serving the client it was built
against — so a hard reload can NEVER pull a newer, possibly-**engine-incompatible**
client onto the old engine (the load-bearing invariant: a new endpoint / event /
migration in a mixed change would break the old-engine + new-client pairing). A
*Switch to new version* respawns the engine; the new process snapshots the
then-current `dist/`, so client and engine advance together.

**Exception — a frontend-only Apply advances the served client in-process** (no
respawn; `crates/lucidos-engine/src/engine/frontend_refresh.rs`,
`docs/plans/2026-07-02-frontend-only-apply-served-in-dev.md`). A pure frontend
change (`files_require_restart == false`) leaves the **engine binary unchanged**,
so a newer client built from that diff IS compatible. On such an Apply the
applying engine waits for the build-watch to republish `dist/` (polls the source
`sw.js` `BUILD_ID` vs the served snapshot's, bounded timeout), pins a **fresh
generation** snapshot, and atomically swaps the handle — so the served `sw.js`
advances and the client refresh badge/toast fire without a manual restart. A
**mixed** change still advances only via a Switch. **The refresh is gated on the
running engine binary being current** (`build_state == Idle` AND the on-disk binary
id matches the running one — `frontend_advance_is_safe`): if a mixed change was
applied but not yet switched, `dist/` already holds a client built for the NEW
engine, so a *later* frontend-only Apply must NOT snapshot it onto the still-old
engine — that gate (checked before the poll AND again before the swap, since a
mixed Apply can land mid-poll) is what preserves the invariant. **On that
deferred branch the engine emits the transient `FrontendUpdateDeferred` event**
(`engine::frontend_refresh::emit_frontend_update_deferred`), so the page surfaces
a keyed "frontend change applies on Switch" hint toast
(`store/actions/engine-update.ts` `handleFrontendUpdateDeferred`) instead of the
user seeing a frontend-only Apply do nothing — the change ships when they Switch.
Coalesced (a later frontend-only Apply supersedes an in-flight refresh);
fail-safe (a failed re-snapshot leaves the current one in place — never a 404);
the superseded generation is removed after a grace delay so an in-flight request
never 404s.

Packaged serves its immutable bundled Resources directly (already one unit — no
snapshot, and the in-process refresh is a no-op there); a failed boot snapshot
falls back to serving the live dir (never a 404). A shared-`dist` rebuild by one
workspace does not IMMEDIATELY change what a *peer* workspace's engine serves —
each engine has its own snapshot, and the applying engine's in-process refresh
runs only in the **applying** engine. But a peer does **eventually catch up on
its own**: a dev-only per-engine periodic task
(`engine::frontend_refresh::spawn_served_frontend_sync`, ~10s) re-snapshots the
shared `dist/` and emits the transient `ServedFrontendAdvanced` event **when — and
only when — advancing is INV-A-safe**. "Safe" is NOT just "my on-disk binary is
unchanged" (`disk == running`): during ANOTHER workspace's *mixed*-change engine
rebuild the on-disk binary stays old for tens of seconds while the build-watch has
already republished `dist/` with a new-engine client, so the disk gate alone would
wrongly drag the peer onto an incompatible client. The load-bearing guard is a
runtime check (`engine_source_matches_head`) that classifies the files changed
since the running engine's commit (`git diff --name-only <running-engine-commit>
HEAD`) with the SAME `files_require_restart` classifier the Apply path uses: no
restart-requiring file ⇒ frontend-only ⇒ advance; a restart-requiring file ⇒ a
mixed change is in flight ⇒ defer to the *Switch* flow (the peer gets the Switch
badge from the shared binary). Reusing that exact classifier — not a coarse
`crates/lucidos-engine` pathspec — is what keeps the gate from stranding a
frontend-only change that also touches a restart-IGNORED engine file (a test
`.rs`, a `.md`). So a
frontend-only Apply in one workspace surfaces the Refresh badge/toast in peer
workspaces without a manual restart, while a mixed change never drags a peer onto a
client for an engine it isn't running. This same git veto also hardens the
applying-engine's own frontend-only path against a *concurrent* peer mixed rebuild.
See `docs/plans/2026-07-03-cross-workspace-frontend-only-refresh.md`.

**Badge ⟺ toast — coupled on ARRIVAL, decoupled on DISMISS (never a badge without
a toast *on arrival*).** Because the served client is always engine-compatible,
`syncClientUpdateFromBuild` — comparing the running bundle's `CLIENT_BUILD_ID`
against the served `sw.js` `BUILD_ID` — is an honest "is my loaded code stale?"
signal that is true *only* when a compatible newer client is actually being served
(i.e. after a switch). The **badge** ("client update available" dot) is
`updateAvailable = stale` — the persistent update affordance. The **toast**
(**"New version available — refresh to sync"**) surfaces when
`stale && !wasSwUpdateDismissed(served)`. So on ARRIVAL (a stale build first
seen, not yet dismissed) badge and toast appear together — a lit badge is never
alone *on arrival*. They decouple only on DISMISS: dismissing the toast (its X or
the **"Later"** defer action) hides the toast and remembers this build, but the
**badge stays lit** so the user can still refresh from the reload badge — the
toast is the announcement, the badge is the affordance. Dismissal is **durable**
(`localStorage`, keyed by build id — survives reload AND cold relaunch), and a
genuinely newer served build re-surfaces the toast. There is **no engine-pending
gate** on the toast — the serving layer, not the toast, upholds "never a client
for a non-running engine". The engine **Switch** surface mirrors this exactly: the
reload-glyph badge (`engineVersionReady = ready`) is the persistent affordance,
and the **"New version available → Switch to new version"** toast
(`engine-update.ts`) surfaces when `ready && !wasSwitchDismissed(disk_build_id)`
(keyed on the on-disk binary build id `version-status.disk_build_id`); a dismiss
defers only the toast (badge persists, remembered durably), and a genuinely newer
on-disk build re-surfaces it. `dismissToast` (store.ts) records the per-build
dismissal but does **not** clear the badge signal.

The post-restart **"Engine restarted"** toast (`connection.ts`) is a plain
action-LESS confirmation — a pure engine-only Apply leaves the served client
byte-identical, so nothing surfaces. When a restart ALSO rebuilt the client (mixed
change), the switched-to engine serves its newer pinned client, so
`syncClientUpdateFromBuild` (re-run after the restart + via the SW nudges) sees the
served `BUILD_ID` differ and surfaces the Refresh toast + dot together. The
`ChangeApplied` arm only nudges the SW (`scheduleServiceWorkerUpdateChecks`) so the
build-id check re-runs promptly once the rebuild lands. The dot also renders on the
**reload icon in the workspace switcher** (control panel), mirroring the brand
toggle's badge. (Two badge-lights are deliberately NOT part of the client check:
the Tauri desktop app-version signal in `connection.ts`, a separate versioned-shell
update mechanism, and the dev-only `import.meta.hot` HMR path in `main.tsx`, inert
under built serving.)

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
which covers a dead watch and the **solo `-b`** rebuild. It also avoids wasteful
rebuilds: the determinism guard (vite.config.ts `lucidos-sw-stamp` hashes asset
names, so identical source → identical `BUILD_ID` → byte-identical `sw.js`) means a
rebuild only changes the id when source actually differs, but the old
`start_frontend_built` did `rm -rf dist` + a fresh build on EVERY startup —
needless I/O on the shared tree. (This *used* to also toast every other workspace
"New version available" because peers served the live `dist/` and their SW saw a
new worker on EVERY rebuild — including a no-op one. That can no longer happen —
each engine serves its own **pinned snapshot** (see the serving section above), and
the peer sync only advances (+ toasts) when the served `sw.js` BUILD_ID *actually*
changed (`source_rebuilt`) AND advancing is INV-A-safe. So a deterministic no-op
rebuild — identical source → identical `BUILD_ID` — never advances a peer or
toasts; only a *genuine* frontend change does, which is the intended
cross-workspace behavior, not the old spurious churn. The reuse rule stands as a
build-I/O efficiency measure.) Teardown is **ref-counted**: `cleanup_processes`
and `stop.sh` call `teardown_shared_build_watch_if_idle`, which kills the watch
only when no workspace of the checkout is still serving the frontend (this
workspace's `frontend.pid` is removed first, so it doesn't count itself).

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
fired, the toast logic is the suspect. Badge and toast are coupled **on arrival**
(the badge is `stale`; the toast surfaces alongside it when
`stale && !dismissed`), so a badge without a toast is a regression **only on
arrival** — after a *dismiss* it is the intended state (the badge persists as the
affordance while the toast is deferred, remembered durably in `localStorage`). So
a lone badge is expected once the user has dismissed for this build; suspect the
toast logic only when a *freshly-served, never-dismissed* build lights the badge
without the toast. Note the served `BUILD_ID` is the running
engine's **pinned** snapshot: a random mid-build shared-`dist` rebuild won't change
it (that's the invariant, not a bug). It advances on a Switch (respawn
re-snapshots) **and** on a **frontend-only Apply** — which re-snapshots in-process
(`engine::frontend_refresh`, engine binary unchanged → compatible), so a
frontend-only change surfaces the badge/toast within a few seconds without a
restart. A **mixed** change still advances only via the Switch.

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
the deferred deps install on the next *full* restart (stop + `web-dev.sh`). The
**`--engine-build` background rebuild** (`ENGINE_BUILD_ONLY`) gets the SAME skip
treatment for the same reason: it too runs `build_sdk` while the frontend is live,
and a hard-fail there aborts the whole background build — which surfaces to the
user as a false "New engine version failed to build" even though the engine binary
compiled fine (the on-disk binary is already written by then, so the next Apply
also mis-fires "New version available" before its build finishes). Both
`ENGINE_ONLY` and `ENGINE_BUILD_ONLY` are the "keep the running frontend alive"
paths, so `ensure_npm_deps` skips the install for either.
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

**e2e runs on a RELEASE engine by default.** `scripts/lib/e2e.sh` (sourced by `e2e.sh` / `e2e-browser.sh` / `e2e-api.sh`) sets `RELEASE=1` so `build_or_find_engine` builds + serves `target/release/lucidos-engine` — the debug engine's CPU cost drove the mobile-webkit contention wedge, and release matches the packaged/prod engine. `LUCIDOS_E2E_DEBUG=1` opts back to the fast debug build for local iteration; `CARGO_BUILD_JOBS` is capped at half the cores on the release path to avoid a codegen OOM. See `.claude/rules/testing.md` and `docs/plans/2026-06-28-e2e-always-release-build.md`.

### macOS code signing (stable TCC grants)

A `cargo build` engine binary is `adhoc, linker-signed`; its CDHash changes every rebuild, so macOS TCC (privacy) discards prior permission grants and re-prompts ("lucidos-engine would like to access …") after each rebuild. `build_or_find_engine` (in `scripts/lib/workspace.sh`) re-signs the freshly built binary with a **stable self-signed identity** (`scripts/lib/codesign.sh` → `sign_engine_binary`), giving it a rebuild-stable Designated Requirement so one Allow click persists. Run `./scripts/dev-codesign-setup.sh` **once** first — it creates + trusts the cert (single GUI password prompt). Until then signing is a no-op and the build proceeds unsigned (with a hint). This only stops the re-prompting; the prompt still names "lucidos-engine" (a post-fork TCC responsibility disclaim to attribute it to Claude Code is not possible — see the note in `runtime/claude_code.rs::build_command`).

**Search-list registration is load-bearing.** `codesign --sign <name>` resolves the identity through the **keychain search list**, not the `--keychain` flag — so the dedicated `lucidos-dev-signing.keychain-db` must be *in the search list* or every sign fails with "no identity found" and silently falls back to ad-hoc (the prompts never stop, even though `find-identity -p codesigning "$KEYCHAIN"` reports the identity as valid). `lucidos_ensure_keychain_in_search_list` (in `codesign.sh`) registers it; both setup and `sign_engine_binary` call it, so existing installs self-heal on the next `-b` build. **Per-binary, not per-workspace:** every engine binary signed with the same identifier (`lucidos-engine`) + same cert leaf shares one Designated Requirement, so a single Allow covers all workspaces. But a binary built outside the scripts — e.g. `cargo run` from an IDE — bypasses `sign_engine_binary` and stays ad-hoc; launch via `web-dev.sh` so it gets signed.

## Build

```bash
cargo build -p lucidos-engine --release    # Engine
cd crates/lucidos-app && cargo tauri build # Desktop app
./scripts/build-dmg.sh                      # macOS: self-contained .app + .dmg (bundled PG)
./scripts/build-dmg.sh --emit-tarball       # macOS: ALSO emit the SIGNED headless lucidos-<version>-<triple>.tar.gz + .sha256
./scripts/build-headless.sh                 # Linux + macOS: Tauri-free headless tarball for the HOST triple
./scripts/build-headless.sh --check         # validate the resource contract (offline)
```

Dev: native engine + Docker PostgreSQL. Production: single Docker container. Makefile: `make build`, `make test`, `make run`.

### Lockfile determinism — builds are fail-closed (ADR 0020)

The committed lockfiles (`Cargo.lock`, root `package-lock.json`) are the single
source of truth for exact dependency versions — the whole tree, direct **and**
transitive. Every build consumes them **strictly**, so a build **errors** rather
than silently rewriting a lockfile on manifest drift:

- All `cargo build|test|check|clippy|run` in `scripts/**` + `Makefile` pass
  **`--locked`** (and `cargo tauri build … -- --locked`). `cargo install tauri-cli`
  already uses `--locked`.
- All npm install sites use **`npm ci`** (never `npm install`) — installs exactly
  the lockfile, verifies integrity hashes, errors on `package.json`↔lock drift.
  `ensure_npm_deps` runs `npm ci` from the workspace root (`install_root`), behind
  its existing fingerprint gate + frontend-running guard.

A dependency version changes ONLY via a deliberate `cargo update` / `npm install
<pkg>` that updates **and commits** the lockfile. **Do not** "fix" a build that
fails with *"the lock file needs to be updated but --locked was passed"* by
dropping `--locked`/reverting to `npm install` — regenerate + commit the lockfile
instead. Manifests keep idiomatic caret/range specifiers (NOT exact `=` pins — see
ADR 0020 for why exact-pinning is the wrong tool).

### CC worktrees get `node_modules` at spawn — don't reinstall it

A CC session must NOT run `npm install` / `npm ci` in its worktree "because
`node_modules` is missing" — the engine already provisioned it before the session
started, and the reinstall is pure waste. The provisioning lives in
`crates/lucidos-engine/src/engine/agent_session/run_session/spawn_context.rs`
(`node_modules_setup::{has_install_marker, member_node_modules_links}`): for every
**Lucidos-source** thread (NOT external-repo, NOT app-coding-agent), on spawn the
engine **hardlinks** main's installed trees into the worktree (`cp -al`, ~1–2s,
zero disk; `cp -a` fallback across filesystems), falling back to a cold
`npm ci --prefer-offline` only when the main repo itself has no install to copy.
It links **two kinds of tree**: the hoisted worktree-ROOT `node_modules`, **and**
each npm workspace-member `node_modules` that exists in main
(`NPM_WORKSPACE_MEMBERS` = the root `package.json` `workspaces`; only
`crates/lucidos-app/node_modules` has its own tree today —
`packages/lucidos-sdk` fully hoists).

Two surprises of the resulting setup that make a session *think* something's
missing when it isn't:

- **A few deps live ONLY in the member tree, not the root.** Most deps hoist to
  `<worktree-root>/node_modules`, but an **un-hoistable** package sits in the
  member's nested `node_modules` instead — notably **`vitest` is at
  `crates/lucidos-app/node_modules/vitest`, NOT the root** (its 4.x tree conflicts
  with a root-hoisted version, so npm nests it; every `@vitest/*` path key in
  `package-lock.json` is `crates/lucidos-app/node_modules/...`). A root-only
  `ls node_modules/vitest` therefore reports "missing" while `npm test` /
  `tsc --noEmit` resolve it fine. (This was the "Cannot find module 'vitest'" /
  `vitest: command not found` breakage before member trees were linked.)
- **The member tree carries NO `.package-lock.json` marker.** npm writes that
  marker only at the install ROOT, never into a member's nested `node_modules`
  (main's `crates/lucidos-app/node_modules` has real packages but no marker). So
  `has_install_marker` is the right check for the root tree only; the member link
  is gated on the source dir *existing* instead. Don't judge the member tree
  "not installed" by that marker's absence.

Verify the root tree with `ls <worktree-root>/node_modules/.package-lock.json` and
the member tree with `ls <worktree-root>/crates/lucidos-app/node_modules/vitest`.

Why it matters beyond wasted minutes: a redundant `npm ci` saturates disk I/O and
was the driver of the mobile-webkit shard-contention wedge
(`docs/plans/2026-06-27-mobile-webkit-shard-contention.md`), and a bare
`npm install` would rewrite the committed `package-lock.json` — the exact
determinism violation the section above forbids (ADR 0020). Frontend tests
(`npm test`, `npx tsc --noEmit`) run against the provisioned tree as-is.

**Shared staging (`scripts/lib/stage_runtime.sh`).** The self-contained runtime
tree — the 6 `RESOURCE_NAMES` (`lucidos-engine`, `lucidos-gateway`, `lucidos` (the
CLI), `frontend`, relocatable **PostgreSQL 18 + pgvector** `postgres`, `sdk`) — is
staged by ONE shared library that both build paths source: target-triple resolution
(`stage_runtime_triple` from `uname`), the theseus-rs PG18 + pgvector
fetch/compile recipe (`stage_runtime_fetch_postgres` — the same code resolves the
macOS `*-apple-darwin` and Linux `*-unknown-linux-gnu` relocatable Postgres asset by
triple; the `PG_SYSROOT` override is applied only on a Darwin host, Linux uses
system gcc), the frontend/binary builds, and the 6-resource `stage_runtime_assemble`.
The pure helpers are offline-tested by `scripts/lib/stage_runtime_test.sh`. The
`lucidos` CLI is **load-bearing**, not a convenience: the engine resolves it as a
sibling of `lucidos-engine` (`find_lucidos_cli_dir`) — or by absolute path via
`LUCIDOS_CLI_BIN` when the launcher stamps it (`desktop.rs::spawn_gateway` /
`service_runtime_env_pairs`) — to launch the Claude Code permission-prompt MCP
server (`lucidos mcp-permission-server`), so a bundle that omits it breaks every
coding-agent thread on its first tool call — the engine now fails the CC spawn fast
with a descriptive error rather than starting a doomed session
(`resolve_lucidos_binary` in `crates/lucidos-engine/src/runtime/claude_code.rs`).

**Headless tarball — macOS signed (`build-dmg.sh --emit-tarball`).** In addition to
the `.app`/`.dmg`, emits a plain per-platform `lucidos-<version>-<target-triple>.tar.gz`
(the 6 `RESOURCE_NAMES`) plus a `shasum -a 256 -c`-compatible `.sha256` sidecar —
the Docker-free, compile-free download artifact a later `install.sh` lays down
(step 1 of `docs/plans/2026-06-30-installer-step1-headless-tarball.md`). It is sourced from
the SIGNED `.app` `Contents/Resources/` (not `bundle-resources/`, whose copies are
never signed), so the Mach-O files inside keep their Developer ID signatures. The
flag is opt-in and applies to any build mode (no-op under `--check` / a build-less
`--release-attach`); default behavior is unchanged when it is absent.

**Headless tarball — Linux + macOS unsigned (`build-headless.sh`).** The Tauri-free
build path (step 2 of `docs/plans/2026-06-30-installer-step2-linux-tarball.md`). Runs the
shared staging for the **host** triple — no `cargo tauri build`, no `.app`, no DMG,
no codesigning — then reuses `headless_tarball_emit` to produce the same
`lucidos-<version>-<triple>.tar.gz` + `.sha256`. On **Linux** this is THE release
build path; on **macOS** it produces an UNSIGNED tarball (use `build-dmg.sh
--emit-tarball` for the signed macOS artifact). It compiles natively, so `--triple`
must equal the host — cross-arch artifacts come from the CI matrix's per-arch
runners. Flags: `--triple`, `--out-dir` (default
`.lucidos/release-staging/<version>/`), `--version` (default RELEASE →
tauri.conf.json → 0.0.0), `--check`. Offline-tested by
`scripts/lib/build_headless_test.sh`.

**Linux tarballs via CI (`.github/workflows/release-tarballs.yml`).** A
`workflow_dispatch` + `v*`-tag-`push` matrix over the four target triples
(`x86_64-unknown-linux-gnu` is the must-work entry; macOS x86_64 + Linux aarch64
are best-effort; `fail-fast: false`). Each entry runs `build-headless.sh` on a
**native** runner — the Linux entries INSIDE an `ubuntu:22.04` container (the
**glibc 2.35 floor**: a binary built on the raw 24.04 runner image refuses to
start on Ubuntu 22.04 / Debian 12 / RHEL 9 with `GLIBC_2.3x not found`, and the
same-machine tarball-smoke can't see it), guarded by an "Assert portability
floor" step that fails the build if any staged binary references a
`GLIBC`/`GLIBCXX`/`CXXABI` symbol version above that floor — and uploads the
tarball + `.sha256` as **workflow artifacts only**.
It does **NOT** auto-publish: it never creates a Release/tag; the optional "attach
to an existing Release" step is gated behind a manual `attach_to_release` input
(default off) **and** a tag ref, and uses `gh release upload` (never `gh release
create`). The signed macOS tarball still ships from the local `build-dmg.sh
--emit-tarball` path; the macOS CI entries are unsigned, for parity/verification.

Packaging lives in `scripts/lib/headless_tarball.sh` (offline-tested by
`headless_tarball_test.sh`); it copies with `ditto` on macOS (preserves the embedded
Mach-O signatures) and `cp -a` elsewhere (Linux runners have no `ditto`).

## Installer (`install.sh` + `uninstall.sh`)

`install.sh` is the user-facing `curl … | sh` installer (steps 3 + 4 of
`docs/plans/2026-06-30-installer-step3-download-and-run.md` +
`docs/plans/2026-06-30-installer-step4-service-mode.md`). It has **three modes**:

- **(default) download-and-run + register a service** — detect the host triple (the
  SAME `stage_runtime_host_triple` map the build scripts use — no divergent mapping),
  resolve the version, `curl` the prebuilt `lucidos-<version>-<triple>.tar.gz` +
  `.sha256`, **verify the checksum (mandatory, fail-closed)**, extract to the SHARED
  `$LUCIDOS_PREFIX/runtime/<stem>/`, then **register the bundled gateway as a
  user-level service** (step 4) so it survives terminal-close + reboot and restarts on
  failure. The service runs `lucidos-gateway` directly with the SAME env
  `crates/lucidos-app/src/desktop.rs::spawn_gateway` sets
  (`LUCIDOS_GATEWAY_PG_BACKEND=embedded`, `LUCIDOS_PG_BIN_DIR`/`LUCIDOS_PG_LIB_DIR`,
  `LUCIDOS_ENGINE_BIN`, `LUCIDOS_STATIC_DIR`, `LUCIDOS_SDK_DIR`, `FASTEMBED_CACHE_DIR`,
  `LUCIDOS_BOOT_WITHOUT_PROVIDER=1`, `LUCIDOS_PACKAGED=1` — emitted once by the pure
  `service_runtime_env_pairs`, shared by the foreground launch + the plist + the unit).
  `--no-service` (`LUCIDOS_NO_SERVICE`) runs the gateway in the **foreground** instead
  (the step-3 behavior). No Docker/Rust/Node/clone/compile.
- **`--dev` / `--source` / `LUCIDOS_FROM_SOURCE=1`** — the legacy compile-from-source
  path, preserved verbatim (toolchain bootstrap, clone/update, `data/.env`, build +
  launch via `scripts/run.sh`). The only network/compile path; **always foreground**
  (never registers a service).
- **`--from-tarball <path>`** — install a LOCAL tarball (offline; e.g. one from
  `build-headless.sh`). Verifies the adjacent `<path>.sha256` if present (fail-closed),
  warns if absent, extracts, and registers the service too (unless `--no-service`).

**Service = the GATEWAY only (ADR 0014).** The service supervises the gateway; the
gateway provisions the embedded Postgres + spawns/supervises the engines itself — never
a service per engine. The gateway ignores SIGTERM and stops gracefully on SIGUSR1
(`crates/lucidos-gateway/src/server.rs`), so the systemd unit sets `KillSignal=SIGUSR1`
+ `KillMode=process` (stop the gateway; leave engines + PG for a relaunch to re-adopt).

**Slug-keyed multi-instance.** Several gateways coexist as named *instances*
(`--name <slug>` / `LUCIDOS_INSTANCE`, default `default`). The **port is a mutable
property**, not the identity, so a re-run with a new `--port` moves an instance. Each
instance owns `<prefix>/<slug>/` (registry + embedded PG + `fastembed/` + `logs/` + a
`port` marker) and a slug-suffixed service id; the **runtime is downloaded once and
SHARED** at `<prefix>/runtime/current`. Slugs `gateway`/`runtime`/`current`/`logs` are
reserved (so a `--name` can't alias the dev gateway's `~/.lucidos/gateway` or the shared
runtime). This is how a terminal install coexists with a dev gateway (5251) and the
packaged `.app` (5252). **Service ids + paths:** launchd
`com.lucidos.gateway.<slug>` at `~/Library/LaunchAgents/` (logs `<prefix>/<slug>/logs/
gateway.{out,err}.log`); systemd `lucidos-gateway-<slug>.service` at
`${XDG_CONFIG_HOME:-~/.config}/systemd/user/` (logs `journalctl --user -u
lucidos-gateway-<slug>`).

**Port resolution (idempotent; port is changeable).** Pinned `--port P`: use P if free
or already this instance's, else **fail closed** (a foreigner holds it). Bare on an
existing instance: reuse its recorded `<data>/port`. Bare on a NEW instance: auto-pick
the first free port from 5252 up (so it steps around a running `.app`). After registering,
a **health check** polls `http(s)://localhost:<port>/~/api/v1/health`
(`LUCIDOS_HEALTH_TIMEOUT`, default 120s; `curl -k`, scheme follows the TLS opt-in
below) and fails loud with a logs hint if it never answers.

**TLS opt-in (`--tls-cert`/`--tls-key`, env `LUCIDOS_TLS_CERT`/`LUCIDOS_TLS_KEY`).**
Both-or-neither, files must exist (fail closed). When supplied, the pairs are appended
to the service/foreground env (`service_tls_env_pairs`) so the bundled gateway serves
**https** — which is what gives a NON-localhost device a secure context (service
worker, PWA install, web push all require one; plain http limits them to localhost).
Works with `tailscale cert` / mkcert / CA certs. Engines still never see `LUCIDOS_TLS_*`
(the gateway strips them — it terminates TLS, ADR 0014), and `restart_via_gateway`
already tolerates the scheme mismatch via `peer_scheme_order()`. Remote reachability is
separate (`--bind` below, or Settings → System → Network access; loopback-only default
unchanged). Like provider creds, TLS is baked from THAT run's flags — a re-run without
them reverts the service to plain http.

**macOS CLT preflight (download / from-tarball paths).** `install.sh` probes
`xcode-select -p` on Darwin and **warns (never dies)** when the Command Line Tools are
absent: chat works, but coding agents / Apply / `run_python` shell out to git + python3,
whose `/usr/bin` shims error until CLT is installed. The engine mirrors this at boot
(`git_preflight` + `python_preflight` in `main.rs`, warn-only) and startup-augments its
own process PATH with the common user-install bin dirs
(`core::user_path::augment_process_path` — Homebrew, `/usr/local/bin`, `~/.local/bin`,
npm-global; dedupe ⇒ no-op on a dev shell PATH) so bare-name tools (`claude`/`codex`
fallbacks, chat bash/python shell-outs, stdio MCP servers) resolve under the launchd
minimal PATH exactly as they do in dev. Agent children additionally get the bundled
`LUCIDOS_PG_BIN_DIR` PATH-prepended (`spawn_env::agent_path_prefixes`) so the
advertised bare `psql -c '…'` works inside coding-agent threads on a packaged install,
mirroring what `workspace_script_env_vars` already did for chat bash/python tools.

**Manager detection + degrade.** macOS → launchd; Linux → systemd `--user` (probed via
`systemctl --user show-environment`) + best-effort `loginctl enable-linger` (announced,
never hard-fails). **No supported manager** (e.g. a container) → **degrade to a
foreground launch** with a clear message, never fail.

**Post-extract validation + preflights.** `finish_install` runs the extracted
`lucidos-gateway --build-id` once (the **execution smoke** — a too-old glibc /
wrong-arch tarball fails AT INSTALL TIME with a distro-floor message pointing at
`--dev`, instead of an opaque service crash-loop) and then warns — never fails —
about missing host runtime deps: `git` (the engine shells out for every git op)
and, on Linux, a system CA bundle (candidate list =
`install_ca_bundle_candidates` in `install_common.sh`; rustls reads the system
store for LLM/model/web-push TLS).

**Remote access (`--bind`) + unit-value escaping.** Default posture stays
loopback + plain http; the final banners print the remote options (SSH tunnel and
`tailscale serve` keep a SECURE origin — which web push + PWA require — with zero
config), and the https half is the TLS opt-in above (`append_tls_env` is ADDITIVE,
so the flag-less env block stays byte-identical to `spawn_gateway`'s contract).
`--bind all|loopback|<IP>` (`LUCIDOS_BIND`) writes the machine-global
`~/.lucidos/network.toml` via `service_write_network_toml` (byte-mirror of the
gateway's own writer, preserves `[engine] inherit`) — **never unit env**, which
would permanently shadow the picker's Settings → Network access knob (env beats
the file). Invalid `--bind` values are refused up front (the gateway would
silently fall back to loopback). systemd unit values are escaped via
`service_systemd_escape_env` (`%%`, `\"`, `\\` — an API key with `%` used to
reach the gateway mangled); launchd's twin is `service_xml_escape`.

**Uninstall.** `uninstall.sh` (and `install.sh --uninstall`, which delegates to it):
`--name <slug>` removes one instance (a bare uninstall removes the sole instance, else
lists), `--all` removes every instance, `--list` shows instances + ports. It stops +
unregisters the service (both launchd + systemd artifacts that exist), gracefully stops
that instance's engines + embedded Postgres, and **keeps all data unless `--purge`**
(prints what it left). `--purge` deletes the instance data dir; `--all --purge` also
deletes the shared runtime. The systemd unit FILE is removed **even when the user
D-Bus session is unreachable** (bare ssh, no `XDG_RUNTIME_DIR`) so an
"uninstalled" service can't resurrect at the next boot; in that case the
possibly-running stack is left alone (a bus-less shell can't stop the gateway,
and killing its engines would only make it respawn them).

**Shared logic, one source of truth.** install.sh **sources**
`scripts/lib/{stage_runtime,headless_tarball,install_common}.sh` (triple/stem/URL) and
`scripts/lib/service.sh` (service templating/detection) from `<self>/scripts/lib` when
run from a checkout; when piped it **fetches** those small pure libs from the same ref
(`${LUCIDOS_INSTALL_URL%/install.sh}/scripts/lib`, overridable via `LUCIDOS_LIB_BASE_URL`)
— never re-implementing any map. `install_common.sh` holds the pure URL/version/dir
helpers; `service.sh` splits **PURE** helpers (identity, paths, plist/unit templating,
manager DECISION + compose decision, env pairs, slug/port validation, port candidates,
uninstall paths) from thin **EFFECTFUL** wrappers (the launchctl/systemctl/curl/kill/
pg_ctl calls, port probing, instance listing) — the offline tests exercise the pure
ones and never the effectful ones.

**Layout.** `LUCIDOS_PREFIX` (default `$HOME/.lucidos`) → shared runtime at
`<prefix>/runtime/lucidos-<version>-<triple>/` + a `<prefix>/runtime/current` symlink;
per-instance data at `<prefix>/<slug>/` (override the single-instance data dir with
`LUCIDOS_GATEWAY_DATA`). **Idempotent:** an already-extracted runtime for the target
version isn't re-downloaded/re-extracted unless `--force` (`LUCIDOS_FORCE`).
`--no-launch` (`LUCIDOS_NO_LAUNCH`) installs without starting or registering.

**Env/flags:** `--name`/`LUCIDOS_INSTANCE`, `--version`/`LUCIDOS_VERSION`,
`--base-url`/`LUCIDOS_RELEASE_BASE_URL`
(default `https://github.com/lucidos-dev/lucidos/releases/download/v<version>`),
`--prefix`/`LUCIDOS_PREFIX`, `--port`/`LUCIDOS_PORT` (default 5252),
`--bind`/`LUCIDOS_BIND`,
`--tls-cert`/`LUCIDOS_TLS_CERT` + `--tls-key`/`LUCIDOS_TLS_KEY` (https opt-in, above),
`--no-service`/`LUCIDOS_NO_SERVICE`, `--force`/`LUCIDOS_FORCE`,
`--no-launch`/`LUCIDOS_NO_LAUNCH`, `--uninstall`/`--list`/`--all`/`--purge`,
`LUCIDOS_HEALTH_TIMEOUT`; provider creds (`OPENAI_API_KEY`/`VERTEX_PROJECT_ID`/
`VERTEX_REGION`) are exported into the foreground gateway and **baked into the service
env (mode 600)** when supplied. (The env-as-flag contract means a dev shell that
exports `LUCIDOS_TLS_CERT/KEY` — every engine-spawned subprocess does — silently
configures TLS on a manual install run; the offline test suites `unset` them.) **Caveat (nothing published yet):** the CI workflow is
artifact-only, so the default download **404s today** — the failure message points at
`--dev` / `--from-tarball`. Offline-tested by `scripts/lib/install_test.sh` (download/
extract path) and `scripts/lib/service_test.sh` (service.sh pure helpers + the
foreground/degrade/register/uninstall wiring, all faked — no real launchd/systemd).
