# 0013 — Multi-workspace via a workspace gateway: one reverse-proxy fronts N always-on per-workspace engine stacks, addressed by path prefix

- **Status**: Accepted — implemented (`crates/lucidos-engine/src/gateway/`, frontend base-path awareness, dev harness, picker + switcher, management CRUD, packaging). Refines 0012. **Refined by [0014](0014-multi-workspace-redesign.md)** (2026-06-17): the gateway moves to its own `lucidos-gateway` crate; `/ws/<id>/` becomes `/<workspace>` behind one reserved sigil namespace; per-workspace Postgres clusters become one shared cluster with a database per workspace; the engine serves the frontend directly and stamps `<base href>` so the gateway is a pure streaming proxy (the dev Vite reverse-proxy is removed). See 0014 for the reversals + rationale.
- **Date**: 2026-06-16

## Context

ADR 0012 established the self-contained desktop app: an always-on engine
service (a launchd LaunchAgent) on a *stable engine port*, the window/PWA as
clients, mobile over Tailscale. That model is **single-workspace** by
construction — `crates/lucidos-app/src/desktop.rs` hardcodes one workspace at
`<app-data>/workspace` (`run_service`, line 251), one embedded Postgres cluster,
one `lucidos` database, one engine bound to the stable port. The engine bakes
`LUCIDOS_WORKSPACE` + `DATABASE_URL` at process spawn, so it is single-tenant —
there is no way to re-point a running engine at another workspace.

The user asked to (1) make the client multi-workspace, (2) switch *and create*
workspaces from the app, (3) stop the DMG from silently creating the implicit
`workspace` workspace and instead start from a named one. During grilling the
requirement sharpened past "switch the active workspace" to **use multiple
workspaces concurrently from one client, on both Tauri and web/mobile** — which,
over a single Tailscale origin, rules out a single global "active engine" route.

## Decision

1. **The always-on service becomes a `workspace gateway`** — a new
   `lucidos-engine --gateway` mode (headless, launchd-run on the stable port,
   used in dev too). It owns a **workspace registry**
   (`<app-data>/config/workspaces.json`: stable id → `{display name, dir,
   loopback port}`), **provisions and supervises one Postgres + engine stack per
   workspace** (each engine bound **loopback-only**), and **reverse-proxies
   `/ws/<id>/*`** to the selected workspace's engine.
2. **All workspaces stay always-on, concurrently.** Every registered workspace
   gets its own resident engine + Postgres, so triggers, scheduled tasks, agent
   sessions, and push keep firing headless in *all* of them — not just an
   "active" one.
3. **Clients address workspaces by path prefix**, so one client (a Tauri window
   or a web/PWA over **one** Tailscale `serve` mapping) uses several workspaces
   at once (different tabs/windows under `/ws/<id>/`). The frontend is made
   **base-path-aware** so one bundle serves under `/` and `/ws/<id>/`; the PWA
   service worker scopes per `/ws/<id>/`.
4. **First run auto-creates a workspace named `default`** and drops the user in —
   no blocking name prompt. The old implicit `<app-data>/workspace` is not
   migrated (no real data exists there).
5. **The gateway serves the root workspace picker + a control API**
   (list/create/rename/delete). v1 management: create (provision a stack),
   rename (registry-only edit, enabled by a stable-id/display-name split),
   delete-to-trash (stop → unregister → move dir to `deleted/`, behind a
   type-the-name confirm). A switcher in the app chrome jumps between workspaces.
6. **Failure is isolated.** Each stack boots independently; the gateway stays up
   even with zero healthy workspaces (the picker is always reachable). A stack
   that fails to boot or crash-loops shows as **unhealthy** in the picker
   (retry / view-logs / delete) without poisoning peers or the gateway.

## Rationale

- **The engine is single-tenant; do not fight it.** `LUCIDOS_WORKSPACE` +
  `DATABASE_URL` are baked at spawn. Making the engine multi-tenant would be a
  deep, risky refactor of every projection, EventBus, and scheduler assumption.
  N isolated single-tenant engines is exactly the dev model
  (`scripts/lib/ports.sh` already runs concurrent per-workspace engines + DB
  containers) — the gateway just makes that model first-class and headless.
- **A gateway is the only thing that satisfies "concurrent, both Tauri and
  web."** Native Tauri-only commands can't serve the PWA; per-workspace ports
  can't traverse one Tailscale origin cleanly. A reverse proxy with per-workspace
  path routing gives one origin, concurrent access, and identical behavior across
  clients.
- **Reusing the engine binary (`--gateway`) beats a third binary or app-crate
  proxy.** The engine already has axum, SSE-proxy patterns
  (`engine/http/workspace_client.rs`), health, and migrations. A `--gateway` mode
  reuses all of it, runs headless under launchd, and works in dev. A dedicated
  crate or a Tauri-service-role proxy would duplicate that plumbing and (for the
  app crate) be packaged-only.
- **Failure isolation is the lived requirement.** The user's own packaged
  workspace "never worked" with no in-app recovery. A gateway that survives a
  dead stack and surfaces health turns that into a one-click retry/delete.
- **Always-on for all preserves the product promise.** Lucidos's value is
  unattended work; a workspace whose triggers stop when you look away is a
  regression. The cost (N resident clusters) is acceptable on a personal machine
  and matches what dev already runs.

## Consequences

- **Engine gains a `--gateway` mode**: registry I/O, stack provisioning +
  supervision, a reverse proxy for `/ws/<id>/*`, a control API, and the root
  picker. launchd runs `lucidos-engine --gateway`; the Tauri app becomes a pure
  client. The per-workspace engine moves to a **loopback-only** bind; the stable
  port now belongs to the gateway. `<app-data>/config/engine-port` is replaced by
  the registry's per-workspace port field (the stable port stays the gateway's).
- **Frontend becomes base-path-aware** (`API_BASE` / `apiUrl()` derive from the
  current URL); the service worker registers + scopes under `/ws/<id>/`. A
  consequence is **per-workspace push subscriptions + SW** — desirable
  (per-workspace notifications), but the push/presence plumbing must key on the
  scoped origin.
- **Dev is reworked, not bypassed** (the user chose dev parity): `web-dev.sh`,
  the ports model, and the e2e harness move to `--gateway` + `/ws/<id>/`. Base-
  path/SW-scope bugs then surface in daily dev.
- **`desktop.rs` shrinks**: stack lifecycle moves into the gateway; the Tauri
  service role becomes "ensure gateway installed + running, point window at it".
- **`mobile.rs` barely changes**: `tailscale serve https / → 127.0.0.1:<gateway
  port>` already maps the root; the gateway just sits behind it. Security posture
  is unchanged — the engine still has no inbound API auth, so the network-
  reachable port (now the gateway, fronting all workspaces under one trust
  boundary) stays **tailnet-private** (`serve`, never `funnel`).
- **Version skew across workspaces is allowed and safe** in dev (isolated DBs →
  independent migrations; the gateway is transport-only). Packaged updates
  rolling-restart stacks to the new binary for a uniform user version. See the
  design doc's "Engine version skew" section.
- **Glossary**: a new `Workspace gateway` entry (dev) lands **with the
  implementation**, refining the `Always-on engine service` + `Stable engine
  port` entries and the user-facing `Workspace` entry. The wording is locked in
  the design doc; it is deliberately not added to `docs/glossary.md` ahead of the
  code, to avoid the code/glossary drift `/harden` flags.

## Alternatives considered

- **One engine, only the active workspace live (switch = restart engine
  elsewhere).** Rejected — breaks always-on for every non-active workspace
  (triggers/push go dormant) and still can't do concurrent use.
- **Multi-tenant engine (one process hosts N workspaces).** Rejected — a deep
  refactor of EventBus/projections/scheduler that all assume one workspace; high
  risk for no extra user benefit over N isolated engines.
- **One LaunchAgent per workspace.** Rejected — N plists to manage, "Quit
  Lucidos" must bootout all, and it still needs a gateway for one-origin
  concurrent web access. One supervisor (the gateway) is simpler.
- **Control-plane API only; clients connect directly to per-workspace ports.**
  Rejected — multi-origin switching is awkward off-LAN (needs a Tailscale serve
  mapping per workspace) and splits behavior between Tauri and web.
- **Subdomain per workspace (`a.lucidos.ts.net`).** Rejected for v1 — cleaner
  origin isolation but needs wildcard TLS + per-subdomain serve, materially
  harder to set up off-LAN than path prefixes.
- **Name-is-the-directory identity.** Rejected — rename would move a live pgdata
  cluster + a git tree with absolute-path worktree links; names would have to be
  filesystem-safe. A stable id + display name makes rename a registry edit.
- **Prompt for a workspace name on first run.** Rejected as the default — adds a
  blocking step before any value is visible; `default` + rename-later is
  zero-friction.
- **Gateway in dev = packaged-only, frontend base-path-aware only.** Considered;
  the user chose full dev parity instead so base-path/SW bugs surface in daily
  dev, accepting the dev-harness rework.
