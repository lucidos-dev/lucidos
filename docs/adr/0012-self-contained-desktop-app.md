# 0012 — Self-contained desktop app: launcher owns Postgres + engine; engine serves the UI; auto-update via GitHub Releases

- **Status**: Accepted (lifecycle refined 2026-06-16 — see note below)
- **Date**: 2026-06-16

> **Refinement (2026-06-16).** The initial foundation coupled the engine +
> Postgres lifecycle to the app window (boot on launch, tear down on exit). That
> is wrong for Lucidos's always-on model: closing the window would stop triggers,
> scheduled tasks, coding-agent sessions, and mobile push. The engine is a
> **persistent service** (a macOS launchd LaunchAgent: start at login, restart on
> crash, headless); the desktop window and the mobile PWA are **clients** that
> open/close against it. Mobile reaches it over **Tailscale** (`tailscale serve`
> → tailnet-private HTTPS; not `funnel`, since the engine has no inbound API
> auth). Details + the auto-setup reality: `docs/desktop-app.md` § Always-on
> service + mobile access. **Implemented 2026-06-16** in
> `crates/lucidos-app/src/{desktop,mobile}.rs` (LaunchAgent service, stable port,
> window-as-client, Tailscale setup) with the packaged restart routing in the
> engine `/api/v1/health` + `/api/v1/restart` and **Settings → Mobile Access** in
> the frontend; full runtime verification needs a real `.app` build on a Mac.

## Context

CLAUDE.md § One-Click Install describes the end state: a self-contained desktop
app (macOS `.app`/`.dmg`) that needs no terminal, Docker, or dev tools, bundling
PostgreSQL+pgvector, the engine binary, and the static frontend. Until now only a
feasibility spike existed (`scripts/prototype/desktop-pg-pgvector-spike.sh`,
proving a relocatable PG + pgvector runs over TCP) and a thin Tauri shell that
pointed a webview at an externally-started engine. The user asked for a real
`.dmg` that also auto-updates ("update available → restart") served from GitHub
Releases.

Several structural questions had to be decided to wire this up.

## Decision

1. **The desktop launcher (the Tauri app) owns the embedded Postgres + engine
   lifecycle.** `crates/lucidos-app/src/desktop.rs` boots a bundled relocatable
   PostgreSQL (initdb on first run, `pg_ctl` start on a free loopback port) and
   spawns the bundled engine as a child process, then tears both down on exit.
   The engine stays a pure "connect to `DATABASE_URL` over TCP" component — no
   OS-specific Postgres management inside the engine.
2. **The engine serves the bundled frontend** behind `LUCIDOS_STATIC_DIR` (SPA
   fallback to `index.html`). The webview navigates to the engine URL, so the UI,
   the HTTP API, SSE, and the service worker are all **same-origin**.
3. **First-run before a provider exists uses `LUCIDOS_BOOT_WITHOUT_PROVIDER`.** The
   engine normally panics with no LLM provider; the launcher sets this flag so a
   packaged build boots instead of crashing. It boots into an
   `UnconfiguredProvider` sentinel that returns a clear "No LLM provider
   configured — add one in Settings → Providers" error on chat and reports
   `llm_configured: false` on `/health` (which drives first-run provider
   onboarding). The user configures a provider in Settings → Providers, then
   restarts into the real one. Dev/docker keep the fail-fast panic.

   > **Update (v0.12.0):** the flag was originally named `LUCIDOS_FALLBACK_MOCK`
   > and booted into the deterministic `MockProvider`, which streamed a fixed
   > pangram — a shipped release was serving mock output to real users. It now
   > boots the `UnconfiguredProvider` (never mock) and was renamed accordingly.
   > `MockProvider` stays reachable only via the explicit `LUCIDOS_MODEL=mock`
   > E2E opt-in.
4. **Auto-update via `tauri-plugin-updater` against GitHub Releases.** The app
   checks a `latest.json` endpoint on launch and prompts to restart. The `.dmg`
   is the first-install artifact; the updater ships `.app.tar.gz` + `.sig` +
   `latest.json` on the same Release. Updater signing (minisign) is **separate
   from** Apple notarization.
5. **pgvector is compiled per-platform in the build pipeline, never on the user's
   machine.** `scripts/build-dmg.sh` fetches the theseus-rs relocatable PG and
   compiles pgvector against it, bundling the result as Tauri `resources`.

## Rationale

- **Launcher-owns-PG keeps the engine portable.** The engine already runs in
  three contexts (dev with Docker PG, docker-compose single container, desktop).
  Teaching it to spawn/manage an OS-specific Postgres would couple it to one
  context and contradict CLAUDE.md's "engine connects to Postgres over TCP". The
  launcher is the desktop-only seam, so PG lifecycle belongs there.
- **Same-origin via engine static-serving avoids a class of bugs.** Letting Tauri
  serve `dist` and having the frontend cross-origin-call the engine would put
  SSE, cookies, and the service worker across an origin boundary
  (`tauri://localhost` → `http://127.0.0.1:port`). Serving the frontend from the
  engine and navigating the window there keeps everything same-origin, exactly
  like the dev reverse-proxy. It also gives the docker-compose path a real UI for
  free.
- **A boot-time gate beats forcing `LUCIDOS_MODEL=mock`.** Forcing mock in the
  launcher would pin mock permanently — even after the user adds a key — and (the
  original `LUCIDOS_FALLBACK_MOCK` bug) serve a fixed pangram as if it were a real
  answer. `LUCIDOS_BOOT_WITHOUT_PROVIDER` only applies when nothing is configured
  and boots a clear no-provider state (not mock), so the next launch picks up the
  real provider and a first run never serves fake output.
- **GitHub Releases + `tauri-plugin-updater` is the first-class path.** It is the
  supported Tauri updater backend, integrates with the bundle, and needs no
  separate update server.

## Consequences

- The build pipeline (`scripts/build-dmg.sh`) must compile pgvector per platform,
  and shipping requires an Apple Developer ID (notarization) + a Tauri updater
  signing key + GitHub Releases publishing. These credentialed steps are env-gated
  and documented in `docs/desktop-app.md`; they cannot be run from a CC worktree.
- Workspace state (the Postgres cluster + `data/`) lives under the OS app-data dir
  so it survives updates (the updater replaces the `.app`, not app-data).
- The engine gains a small static-serving branch (`LUCIDOS_STATIC_DIR`) and a
  `LUCIDOS_BOOT_WITHOUT_PROVIDER` boot path — both inert in dev/docker.
- Dev is unchanged: `scripts/tauri-dev.sh` still uses Docker PG + a native engine;
  `desktop::launch` / the updater short-circuit on `tauri::is_dev()`.

## Alternatives considered

- **Engine boots Postgres itself.** Rejected — couples OS-specific process
  management into a component that also runs against Docker/external PG, and
  violates the "engine connects over TCP" rule.
- **Tauri serves `dist`; frontend cross-origin-calls the engine.** Rejected —
  SSE/service-worker/origin friction across `tauri://` → `http://127.0.0.1`. Same-
  origin engine serving is simpler and reuses the dev model.
- **Force `LUCIDOS_MODEL=mock` in the launcher.** Rejected — permanently pins mock
  even after a real provider is configured.
- **A custom/Sparkle updater or a bespoke update server.** Rejected —
  `tauri-plugin-updater` + GitHub Releases is first-class and needs no extra
  infrastructure.
- **Ship `docker-compose` with the desktop app.** Rejected — requires Docker on
  the user's machine, defeating the one-click goal.
