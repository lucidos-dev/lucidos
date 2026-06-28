# 0014 — Multi-workspace redesign: standalone gateway crate, `/<workspace>` path prefix, shared Postgres cluster, engine-served frontend

- **Status**: Accepted — **partially implemented** (2026-06-18). Done: the standalone `lucidos-gateway` crate (no engine dep), `/<slug>/` + `/~/` sigil routing as a pure streaming proxy with `X-Forwarded-Prefix`, the engine serving `dist/` directly + stamping `<base href>` (the dev Vite proxy / `dev_proxy.rs` removed), frontend base-path-from-`<base>` + SW scoping, the dev scripts (`web-dev.sh`/`stop.sh`/`status.sh`/`lib/{workspace,e2e}.sh`), (2026-06-17 refinement) a **single shared dev gateway on a fixed port** with a machine-global durable registry + a **per-workspace `autostart`** flag and picker toggle (see "Dev runtime topology" + §10), (2026-06-18 packaging pass) `desktop.rs` + `build-dmg.sh` wiring so packaged `Lucidos --service` boots bundled `lucidos-gateway`, passes it the bundled `lucidos-engine`, and signs/verifies both resource binaries, and the **one-shared-Postgres-cluster move** (§6) with verify-then-decommission migration from legacy per-workspace databases (§7). Legacy registry `database_url` values are now migration sources only; steady state is one shared cluster and one database per workspace. Refines and partially supersedes 0013.
- **Date**: 2026-06-17

## Context

ADR 0013 shipped multi-workspace via a *workspace gateway*: `lucidos-engine --gateway` fronts N always-on per-workspace engine stacks, addressed by path prefix `/ws/<id>/`, each workspace on its own Postgres cluster (a Docker container per workspace in dev, a separate embedded `pgdata` cluster per workspace packaged), with the engine reverse-proxying the frontend to Vite in dev (`dev_proxy.rs`). It works, but a grilling session surfaced four changes the user wanted plus one the design dialogue produced:

1. **Prettier URLs** — `https://host/dev` (the workspace as the first path segment), picker at `https://host/`, no `/ws/` prefix — and a scheme an external web server could later wrap with `/{user}` access control.
2. **One Postgres server hosting many databases** (not a cluster per workspace) — to cut the memory cost of N postmasters on a personal machine and in the one-click install.
3. **Remove Vite from the serving path.**
4. (raised mid-grill) **Give the gateway its own crate**, not a mode of the engine binary.

It also folds in the user's gateway bug report: a gzip 502 worked around by stripping `Accept-Encoding` (Issue 1), the `dev_proxy.rs` root cause (Issue 2), the HTML-rewrite-needs-uncompressed-bytes constraint (Issue 3), browser-side compression lost as a side effect (Issue 4), a speculative retry merged on a misdiagnosis (Issue 5), and boot-window 502s (Issue 7).

The security model is **unchanged and load-bearing**: the engine has no inbound API auth; the whole posture is one trust boundary, tailnet-private (`serve`, never `funnel`).

## Decision

1. **Standalone `lucidos-gateway` crate + binary, with no dependency on `lucidos-engine`.** The reverse-proxy / control-plane / registry / supervision / Postgres-provisioning code moves out of `crates/lucidos-engine/src/gateway/` into its own crate. The small shared surface it needs (the `log!` macro, hop-by-hop/proxy helpers, TLS bootstrap, `LUCIDOS_RELEASE`, the Postgres readiness probe) is extracted to a tiny shared util or duplicated — the crate must not pull in the engine's heavy core. The gateway spawns the engine binary by path via the existing `LUCIDOS_ENGINE_BIN` (its own `current_exe` is now the gateway, so the path must be explicit). Packaging: `Lucidos --service` spawns `lucidos-gateway`, which spawns one `lucidos-engine` per workspace.

2. **Path-prefix routing `/<slug>/` replaces `/ws/<id>/`.** Workspaces are addressed at `/dev`, `/personal`; the picker is at `/`. All gateway-owned surface (picker assets, control API, health) lives behind **one reserved sigil namespace** — `~` — at `/~/…`. The only naming rule is that a workspace slug **cannot start with the sigil**; there is no growing forbidden-word list to keep in lockstep with root assets. The workspace identity stays the stable id + display name from 0013 (rename = display-name edit; the slug/id is stable).

3. **The gateway is a pure strip-and-forward streaming proxy.** On the way in it strips `/<slug>` and adds an `X-Forwarded-Prefix: /<slug>/` request header; on the way out it forwards the response **untouched** (full streaming, compression intact). It no longer reads or rewrites `text/html`, no longer strips `Accept-Encoding`, and no longer carries the speculative stale-keepalive retry. This removes the root cause of Issues 1/3/4 (and the retry of Issue 5).

4. **The engine serves the built frontend directly and stamps `<base href>` per request.** The engine serves `dist/` directly via the static-serve path in **both dev and packaged** (no proxy). When it serves `index.html` it injects `<base href="/<slug>/">` derived from the `X-Forwarded-Prefix` header; absent that header (a direct hit on the engine's own port), it stamps `/`. The browser therefore keeps the workspace prefix on its own follow-up asset requests, so no gateway-side HTML rewriting is needed. One engine serves both gateway-fronted and legacy-direct access correctly, per request.

5. **Vite becomes build-only; the dev reverse-proxy and HMR are removed.** `vite build --watch` stays as the bundler; the engine serves the resulting `dist/` directly (mirroring packaged). `dev_proxy.rs` and the engine→Vite proxy are deleted (Issue 2), and the `--hmr` live-dev-server path is dropped — one serving path everywhere.

6. **One shared Postgres cluster, one database per workspace.** The gateway brings up a single cluster (dev: one shared Docker container; packaged: one embedded cluster under `<app-data>/pgdata`) and provisions a database per workspace (`CREATE DATABASE lucidos_<wsid>`). Each engine still receives a single-tenant `DATABASE_URL` (`…/lucidos_<wsid>`) — **the engine is unchanged.** Workspace delete becomes `DROP DATABASE` (the shared cluster is never torn down for one workspace).

7. **Existing data is migrated, not recreated, and the old clusters are kept until verified.** A one-time migration: `pg_dump` each existing per-workspace `lucidos` db → `CREATE DATABASE lucidos_<wsid>` on the shared cluster + restore → re-point each engine. The old per-workspace containers are **left intact as the rollback path**; a separate, explicit decommission step removes them **only after** the shared cluster is verified working (mirrors the codebase's delete-to-trash / recoverable-volume convention).

8. **Legacy direct-port access stays a first-class mode.** Hitting an engine directly on its port (no gateway, `LUCIDOS_NO_GATEWAY`) keeps working; with no `X-Forwarded-Prefix` the base stamp is `/`, so it is correct for free.

9. **No auth in our code, ever — multi-user is an external concern.** The gateway routes purely on path prefix, statelessly, with no notion of identity. A hosted/cloud deployment puts a web server or SSO proxy in front to authenticate and enforce `/{user}`; our URL scheme *enables* that without us building login, sessions, or per-user authz. Posture stays single-trust-boundary, tailnet-private.

10. **Root `/` behavior is smart.** One workspace → drop the user straight in; multiple → show the picker. (Consistent with 0013's first-run "drop the user in".)

11. **Boot-window UX.** During engine cold boot (pgvector init, migrations, embedding warmup, and ~20 coding-agent sessions resuming after a restart), the gateway serves a lightweight "workspace starting…" auto-retry page instead of a raw 502 (Issue 7). The page narrates **boot-phase progress** rather than a single opaque label: the gateway shows the phases it observes itself (provisioning database, starting engine), and the engine — whose own HTTP isn't up yet — reports its internal phases (migrating, downloading the memory model, recovering) best-effort to `POST /~/api/v1/control/workspaces/:id/boot-phase`, which the splash renders on the next 2s refresh. The phase is cleared when the workspace goes healthy or is stopped, so a later cold open starts clean. (The "downloading memory model" phase is the long pole on a first-ever open — the embedding model that powers vector memory is downloaded once, then cached.)

## Dev runtime topology (normative)

This section is **load-bearing and normative** — it exists because an
implementation pass conflated the *packaged* posture (§1: "engines are
loopback-only, the gateway is the sole network-facing surface") with the *dev*
layout and made the dev engine loopback-only, which silently broke §4's
"a direct hit on the engine's own port". **Dev ≠ packaged. Loopback-only is a
packaged concern only.** Implement the two layouts from this table, not from the
repeated security line:

| | **Packaged** | **Dev (`web-dev.sh`)** |
|---|---|---|
| Gateway binds | the stable port (5252), all interfaces | a **fixed** port (**5251**; override `LUCIDOS_DEV_GATEWAY_PORT`), all interfaces — **ONE shared gateway per machine**, NOT one per workspace. Dev uses **5251** so it coexists with a packaged `Lucidos.app` on 5252 |
| Engine binds | **loopback only** (`LUCIDOS_BIND_LOOPBACK=1`) — unreachable except via the gateway | **all interfaces** on the user-facing port (`VITE_PORT`, 5173+offset) — still per-workspace so engines coexist |
| Engine TLS | none (plain http; the gateway terminates TLS) | its own cert (serves https directly) when certs exist |
| Direct app URL | — (engine not network-reachable) | **`https://localhost:5173/`** → engine, base `/` (works exactly as the pre-gateway engine did) |
| Workspace via gateway | `https://<stable>/<slug>/` | `https://localhost:5251/<slug>/` → gateway proxies to the engine |
| Picker | `https://<stable>/~/` | `https://localhost:5251/~/` (or `https://localhost:5251/` — smart root) |

**One shared gateway, durable registry, per-workspace auto-start** (refines §10;
corrects an implementation that scoped the dev gateway per-workspace — each
`web-dev.sh` launch ran its OWN gateway on `API_PORT` (3000+offset) with a
registry holding only that one workspace, so the picker never listed the others,
contradicting §10). The corrected model:

- The dev gateway's app-data (registry, pidfile, log) is **machine-global**
  (`$HOME/.lucidos/gateway`), and it binds a **fixed** port — so every
  `web-dev.sh` launch reuses the SAME gateway and accumulates into ONE registry.
  `seed_gateway_registry` adds/refreshes only the launched workspace's runtime
  fields (`dir`/`port`) and **preserves** a picker-set display `name` +
  `autostart` flag (so a rename/toggle sticks across relaunch). It removes any
  legacy `database_url` after migration verification; the shared database URL is
  derived from the workspace slug.
- **Registry membership = "all ever launched"** — a workspace stays listed in the
  picker after it stops (`stop.sh` POSTs the gateway's `/stop` control API, which
  drops the runtime stack so the supervisor won't respawn it but KEEPS the
  registry entry; it never kills the shared gateway). A `delete` (picker, with the
  type-the-name confirm) is the only thing that removes an entry.
- Each `Workspace` carries an **`autostart`** flag (registry field, picker
  toggle). On gateway boot, per workspace: an engine already answering health is
  **re-adopted**; else an `autostart` workspace is **spawned**; else it is left
  **stopped** (listed, started only on explicit open/launch). New workspaces
  default `autostart=false` (manual, enabled only via the picker toggle).
  **First run** finds an empty registry, creates **nothing**, and the smart root
  serves the **picker** so the user names their first workspace ("personal" /
  "work" suggestions). *(Updated 2026-06-24: first run shows the picker rather
  than auto-creating an `autostart=true` `default` workspace — there is no longer
  any auto-created `default`.)*
- A document navigation to a registered-but-stopped workspace (`/<slug>/`)
  **lazy-starts** it and serves the existing boot-window page (§11) — so the
  picker's "Open" and a direct URL both work on a stopped workspace. API/SSE/
  asset traffic from an already-open tab does **not** lazy-start the workspace;
  otherwise an explicit stop would be immediately undone by background retries.
  New control endpoints:
  `POST /~/api/v1/control/workspaces/:id/{stop,autostart}`.

Both dev URLs are live **simultaneously** — this is §4's "one engine serves both
gateway-fronted and legacy-direct access correctly, per request" made concrete.
The engine stamps `<base href="/<slug>/">` when it sees `X-Forwarded-Prefix`
(gateway path) and `/` when it doesn't (direct hit), so the *same* running engine
serves both. The gateway, spawning a non-loopback dev engine that serves https on
its own port, proxies + health-probes it over **https** (accepting its
self-signed cert). The knobs: the gateway reads `LUCIDOS_GATEWAY_ENGINE_LOOPBACK`
(default `1`; dev sets `0`) to pick the engine's bind + TLS handling;
`scripts/lib/workspace.sh::swap_ports` assigns `ENGINE_PORT=VITE_PORT` (direct)
and `GATEWAY_PORT=API_PORT` (gateway). `LUCIDOS_NO_GATEWAY` is a third, separate
mode (engine only, no gateway at all) — not the same as the dev engine's direct
access, which coexists with the gateway.

## Rationale

- **A standalone gateway shrinks the only network-facing surface.** The gateway is the sole process bound to all interfaces; engines are loopback-only. As a mode of the engine binary it nonetheless *contains* the entire engine (LLM clients, agent loop, EventBus, all of sqlx, every credential path). A separate crate with no engine dependency means the open port runs only proxy + supervise + registry code — a real surface-area win that 0013's "reuse the engine binary" rationale didn't weigh, plus faster independent iteration. This reverses that part of 0013 deliberately.
- **One reserved sigil beats a reserved word-list.** Dropping `/ws/` makes the first path segment ambiguous between a workspace and the gateway's own root files. A fixed word-list (`api`, `assets`, `sw.js`, …) must stay in lockstep with every root asset the shared bundle references — a drift footgun where a new root asset can silently shadow an existing workspace. A single sigil namespace makes the impossible state impossible: one rule, no list.
- **Engine-stamped base + streaming gateway is the actual root-cause fix.** 0013's gateway rewrote root-absolute refs in every HTML response, which forced it to read bodies as text and therefore strip compression globally (Issues 1/3/4). Moving the one needed adjustment — a `<base href>` — into the engine (which now serves `dist/` directly and knows its own prefix from a forwarded header) lets the gateway stop touching bodies entirely: it streams, compression survives, and it stays dumb. The user's own mental model ("just strip `/dev` and forward") becomes literally true.
- **Removing the dev reverse-proxy unifies dev with packaged.** Serving `dist/` directly in dev — exactly what packaged already does — deletes the `dev_proxy.rs` code path that caused the gzip/empty-reply failures, and removes a dev/packaged divergence. HMR is the only thing lost, and "we do real reloads now anyway" (Apply → rebuild → SW refresh toast) makes that acceptable.
- **Shared cluster is the memory win with no engine change.** N postmasters (each with shared_buffers, WAL writer, background workers) is the bloat. One cluster + a database per workspace keeps the engine single-tenant and untouched, and pulls packaged back toward the proven single-cluster boot path of 0012. The cost — a shared failure domain — is acceptable on a supervised, personal machine.
- **Statelessness + path-prefix is what makes external multi-user possible.** Because the gateway carries no identity and routes on path alone, an operator can front it with any auth layer and map `/{user}` themselves. Building auth into our code would be a large, separate project for no single-machine benefit.

## Consequences

- **Build/ship pipeline gains a second binary** (`lucidos-gateway`) to compile, **code-sign** (the macOS stable-identity signing now covers two binaries), and version. Packaging and the dev scripts (`web-dev.sh`, `stop.sh`, status) update to launch/stop the gateway crate and to bring up one shared Postgres container.
- **`gateway/proxy.rs` loses `rescope_html`, the `Accept-Encoding` strip, and the `proxy_retry` integration**; it becomes a streaming forwarder that injects one request header. `dev_proxy.rs` is deleted.
- **The engine gains base-path stamping** when serving `index.html` (read `X-Forwarded-Prefix`, default `/`) and serves `dist/` directly in dev.
- **`postgres.rs` changes from cluster-per-workspace to ensure-shared-cluster-once + `CREATE DATABASE` per workspace**; `PgHandle` teardown becomes `DROP DATABASE`. A migration script + a gated decommission step are added.
- **Frontend** ships relative asset refs that resolve against the stamped `<base href>`; the service worker scopes per `/<slug>/` and push subscriptions re-subscribe once (negligible — effectively no installs predate this).
- **Legacy `LUCIDOS_NO_GATEWAY`** stays supported and is now correct for the base path for free.
- **Docs/glossary updates land WITH the implementation, not here** (to avoid the code/glossary drift `/harden` flags, exactly as 0013 handled it): update `Workspace gateway`, `Workspace`, `Always-on engine service`, and `Stable engine port`, and add a new **sigil namespace** entry. This ADR's index entry notes 0013 is refined by 0014.
- **The tooling-routing fix lands first** (separate thread): the CLI / permission server / `ask_user_question` hook resolving the engine's loopback port instead of the gateway port, so `/harden` and question/permission cards work during and after this redesign.

## Alternatives considered

- **Reserved word-list instead of a sigil namespace (Q2).** Rejected — must track every root asset the shared bundle references; a later root asset can silently shadow a workspace named that.
- **Gateway stamps the base tag, or keeps the full HTML rewrite (base-path).** Rejected — a gateway that touches HTML can't stay a pure streaming forwarder; the full-rewrite variant also keeps assets uncompressed over Tailscale (Issue 4 persists).
- **Pure client-side base bootstrap (no HTML touch at all).** Considered — the smallest possible gateway, but the inline-script ordering and service-worker-scope edge cases are the riskiest frontend change; engine-stamped `<base>` gets the same dumb gateway with far less risk.
- **Spawn-time base path (`LUCIDOS_BASE_PATH` env) instead of a per-request header.** Rejected — pins an engine to one prefix per launch and serves the wrong base if hit directly; the header makes one engine correct for both access modes.
- **Replace Vite with esbuild (Q-Vite).** Rejected — reimplementing asset hashing, the SW `BUILD_ID` stamp, atomic-dist publish, and the css-staleness guard as esbuild plugins is a large lift with real regression risk for marginal gain over keeping Vite as build-only.
- **Keep `--hmr`.** Rejected — it requires retaining a dev-server proxy path, the very thing serving `dist/` directly removes.
- **Per-workspace clusters everywhere, or shared-in-packaged-only (Q3).** Rejected — keeps the memory bloat, or diverges dev from packaged so DB-provisioning bugs hide until packaging.
- **Recreate workspace data instead of migrating, or migrate-and-delete in one shot.** Rejected — loses real history / leaves no rollback; a verify-then-decommission migration is safe.
- **Gateway as a separate crate that still depends on `lucidos-engine` (Q-new B).** Rejected — enforces a module boundary but the binary still links the whole engine, so no surface-area, binary-size, or compile win.
- **Full multi-user with auth now (Q1).** Rejected — login/sessions/per-user isolation is a separate project; external infra can layer it on the path-prefix scheme.
- **Subdomain per workspace (`a.lucidos.ts.net`).** Rejected (carried from 0013) — cleaner origin isolation but needs wildcard TLS + per-subdomain `serve`, materially harder off-LAN than path prefixes.

## Addendum (2026-06-24): supervisor must not cull an alive-but-busy engine

The gateway supervises each engine by probing `/api/v1/health`. The first
implementation respawned an engine (past its boot grace) on a **single** missed
probe with a 3s timeout — it could not tell "dead/wedged" from "alive but too
busy to answer in 3s". Under a heavy load spike (the nightly e2e suite plus a
Playwright WebKit browser pile-up raising memory pressure), healthy engines blew
the 3s budget and were culled; each respawn is expensive (replay ~14k trigger
events, rescan worktrees), which spiked load further and starved the *next*
engine's probe — a self-sustaining cross-workspace **respawn storm** (577
respawns in one night; it interrupted the nightly e2e thread `86ae9ec6`).

**Decision:** the cull policy is now asymmetric and patience-based
(`respawn_decision` in `crates/lucidos-gateway/src/server.rs`):

- The probe classifies its outcome — `Healthy` / `Unreachable` (connection
  refused, non-timeout — a strong "down" signal) / `Slow` (timed out — likely
  alive but busy) / `Other`.
- An engine is culled only after **consecutive** missed probes: a small
  threshold (`DEAD_MISS_THRESHOLD`) for `Unreachable` or a dead process, a high
  one (`SLOW_MISS_THRESHOLD`) for an alive-but-`Slow` engine. The probe timeout
  is 5s. A real death still recovers promptly; a busy engine is never culled on a
  transient spike.
- Boot grace and the restart cap → `Unhealthy` terminal are unchanged.

The browser-orphan half of the loop (force-stopped CC e2e sessions leaking
Playwright browsers across respawns) is tracked separately — see
`docs/plans/2026-06-24-gateway-respawn-storm-fix.md`.

## Addendum (2026-06-27): never respawn an alive engine — supersedes the cull-alive-`Slow` policy

The 2026-06-24 patience fix above made the supervisor *slower* to cull an
alive-but-`Slow` engine, but it still culled one after `SLOW_MISS_THRESHOLD`
consecutive misses. Under **sustained** resource/memory contention (multiple
concurrent `target/debug` engines + CC sessions + the shared Docker Postgres VM,
with the box swapping) a healthy engine misses far more than 5 consecutive probes
— not because it is wedged, but because swapping adds I/O latency so
`/api/v1/health` blows the 5s budget for 30s+ stretches. The supervisor read that
as "wedged", respawned the *live* engine, and each respawn cold-booted (replay
~14k trigger events + a worktree rescan + embedding-model warmup) — spiking
memory/CPU and starving the next engine's probe. Same self-sustaining cascade as
2026-06-24, now leaking through the patient policy (56 engine starts in a day; 8
in 15 min; it interrupted live chat / coding-agent threads —
`Skipping cleanup for thread … — session will resume after restart`).

**Root realisation:** a timed-out HTTP probe cannot distinguish "hung forever"
from "busy right now". The 2026-06-24 fix tried to separate them by *how long*
the engine stays unresponsive, but under sustained contention "busy" outlasts any
fixed threshold, so the heuristic mis-fires on the common case (a contended-but-
fine engine) and pays the worst price (an expensive respawn that feeds the
cascade). Respawning an alive-but-`Slow` engine has negative expected value.

**Decision:** the health supervisor **never respawns a process that is alive** —
booting, busy, `Slow`, or even `Unreachable`-while-alive (apparently wedged). It
respawns **only** a process that has actually exited (`alive == false`), keyed on
`engine_process_alive` (`try_wait` the child / signal-0 the pidfile pid).
`respawn_decision`: after the `Healthy` reset, an alive process returns `Booting`
(inside `BOOT_GRACE`) or `Wait`; only a dead process proceeds to the backoff →
`DEAD_MISS_THRESHOLD` → `RESTART_CAP` → `MarkUnhealthy` path. `SLOW_MISS_THRESHOLD`
and the outcome-asymmetry are removed; `ProbeOutcome` is retained for the
`Healthy` check + log observability.

- **Preserved:** crash recovery + lazy-start (a genuinely dead engine still
  auto-respawns), the boot grace, the restart cap, and explicit restart
  (`restart_workspace` → `respawn_stack`, used by Apply & Restart / control API /
  dev launcher) which is independent of the supervisor.
- **Accepted tradeoff:** a genuinely *deadlocked-but-alive* engine (rare) no
  longer auto-recovers — it waits for a manual restart. Deliberate: never
  interrupting a healthy busy engine is worth more than auto-recovering a rare
  true hang, and the cascade made auto-recovery net-negative anyway.
- **Out of scope (noted):** the underlying memory/CPU contention is an ops
  concern (fewer concurrent debug engines, release builds, Docker VM sizing); and
  the engine's `/api/v1/health` doing two synchronous disk reads per probe
  (`read_engine_version` / `read_app_version`) is a hot-path smell that is now
  harmless (a slow health response no longer culls a live engine), left for a
  follow-up. See `docs/plans/2026-06-27-gateway-never-respawn-alive-engine.md`.
