---
globs:
  - "scripts/**"
  - "**/*.sh"
  - "Makefile"
---

# Scripts & Build

## Dev / runtime scripts

```bash
./scripts/web-dev.sh -w <ws> [-b] [-r] [--hmr]  # Start (-b builds engine; -r release engine; --hmr = live Vite dev server)
./scripts/tauri-dev.sh -w <ws> [-b]       # Start engine + Tauri window
./scripts/stop.sh -w <ws>                 # Stop a specific workspace
./scripts/status.sh                       # Check running status
./scripts/populate.sh -w <ws> [-c]        # Populate test history
./scripts/new-migration.sh <description>  # Create timestamped migration
./scripts/dev-codesign-setup.sh           # One-time: stable macOS code-signing identity
./scripts/test-engine.sh [--full|--fresh] # Engine tests against a dedicated Docker PG
```

### Frontend: built by default; `--hmr` for the live dev server

`web-dev.sh` serves a **built frontend by default**: `start_vite` runs
`npx vite build --watch` (initial build + rebuild on source change) and serves the
bundled `dist/` via `npx vite preview`; the engine reverse-proxies the frontend to
it (`LUCIDOS_DEV_PROXY`). Content-hashed `/assets/*` are cached **cache-first** by
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
changes when rebuilt, so it can't drift ahead of the engine binary. Trade-off: **no HMR** — after a change is applied, `vite build --watch`
rebuilds (a few seconds), the SW detects the new build (each build stamps a fresh
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
`index.html` — `vite preview` then 404s **every** route until the next *successful*
rebuild (which only fires on the next source change), and because the build output
went to `/dev/null` the failure was invisible. Now `start_frontend_built` runs the
watch with `LUCIDOS_ATOMIC_DIST=1`, which makes the `lucidos-atomic-dist-publish`
plugin (`crates/lucidos-app/vite.config.ts`) redirect `build.outDir` to
`dist.staging/` and atomically rename it onto the live `dist/` in `closeBundle` —
which Rollup runs *only after a complete build*. A crashed build never reaches
`closeBundle`, so the last good `dist/` stays in place and preview keeps serving it.
`vite preview` is launched with `--outDir dist` (it must always serve the published
dir, never the staging one). Production builds (`npm run build` / CI / Tauri) run
without the env var → `outDir` stays the default `dist/`, byte-identical to before.
Build output now goes to `crates/lucidos-app/.build-watch/log` (not `/dev/null`),
and the launch's "Waiting for initial frontend build" line prints that path — so a
build failure is one `tail` away instead of an unexplained 404.

**Shared build-watch (checkout-level singleton).** `dist/` (plus `dist.staging`/
`dist.prev`) is a SINGLE directory per checkout — every workspace launched from
the same checkout serves the same `crates/lucidos-app/dist/` through its own
per-port `vite preview`. So the `vite build --watch` that produces it is a
checkout-level singleton, NOT per workspace: its pid + log live at
`crates/lucidos-app/.build-watch/{pid,log}` (gitignored), tracked by
`build_watch_pidfile`/`build_watch_log` in `scripts/lib/workspace.sh`. The first
`--built` workspace to start it owns it; later launches **reuse** it. The reuse
rule lives in `start_frontend_built`: if a healthy watch exists (live pid +
`dist/index.html`) AND either another workspace is already serving this checkout
(`running_frontend_workspaces_in_project` non-empty) OR this isn't an explicit
`-b`, it reuses without rebuilding; otherwise it (re)builds and takes ownership —
which covers a dead/wedged watch and the **solo `-b`** rebuild. This is the fix
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
a wedged shared build-watch (see the stale-CSS guard below) only self-heals via a
**solo `-b`** rebuild — with multiple workspaces sharing it you must stop all of
them (or `kill $(cat crates/lucidos-app/.build-watch/pid)`) before a fresh build
takes over.

**Stale-CSS guard (wedged watch detection).** Because the build-watch survives
engine-only Apply restarts, a single process can run for days across many
rebuilds. Once observed (a 1.5-day-old watch after a large CSS class rename), it
wedged its CSS pipeline — re-emitting fresh JS from changed source while serving a
FROZEN, stale CSS bundle. The served `index.html` then paired new JS (new class
names) with old CSS (old class names), so every renamed/new class had no rule and
the app rendered **unstyled, silently**. The fix is operational (restart the
frontend → clean build), but the desync was invisible until the served JS and CSS
bundles were diffed by hand. The `cssStalenessGuard` plugin
(`crates/lucidos-app/vite.config.ts`, pure logic in `src/dev/cssWedgeDetect.ts`)
now makes it loud: on each build it re-reads the CSS *source* fresh from disk
(bypassing Vite's wedged module cache), normalizes it, and if the source changed
but the emitted CSS bundle did NOT, logs a `[lucidos:css-staleness]` warning to
`build-watch.log` naming the remedy. Scoped to the dev build-watch
(`LUCIDOS_ATOMIC_DIST`), warn-only, try/catch-guarded — production builds stay
byte-identical. **Symptom in the wild:** styling falls off chat/compose surfaces
after an Apply (answered-question options unboxed, control menu renders inline,
icons look greyed) — `grep css-staleness <ws>/.lucidos/build-watch.log`, and if hit,
`stop.sh -w <ws> && web-dev.sh -w <ws>`.

**Debugging a missed toast:** the connection-status popover (control panel) shows
the active SW's `BUILD_ID` as a **Build** row. The page asks the controlling SW for
it via a `lucidos:get-build-id` message (SW replies `lucidos:build-id`), re-querying
on `controllerchange` and each time the panel opens, so the shown id tracks the
*live* worker. If the id is unchanged across workspaces / across an apply, the SW
never picked up a new build (rebuild or stamp issue); if it changed but no toast
fired, the toast logic is the suspect. The live dev server's un-stamped `sw.js`
reports the literal placeholder, shown as `dev`.

`--hmr` (alias `--dev`) opts into the **live Vite dev server** instead: Vite serves
the app as hundreds of unbundled ESM modules with hot module replacement — best for
active frontend iteration, but the SW caches nothing in dev (the shell-cache branch
is gated to built mode via `IS_BUILT`, and `/assets/*` cache-first matches a path
dev never emits) so an iOS PWA cold-loads slowly over the network. Like the build watch, the dev server skips `tsc --noEmit` — type errors
surface at the explicit build / in CC harden.

**Engine-restart interaction (the load-bearing part):** a CC Apply restarts the
engine via `web-dev.sh --engine-only` (`crates/lucidos-engine/src/api/history.rs`),
which sets `ENGINE_ONLY` and **exits before `start_vite`** — so the restart never
touches the frontend. `kill_stale_processes` skips the preview kill when
`ENGINE_ONLY` is set (and never touches the checkout-level shared build-watch on
any per-workspace restart), so the already-running built frontend survives the
restart and the new engine just re-attaches its proxy; the build-watch picks up
the merged source and rebuilds `dist/` on its own. The frontend mode is therefore
chosen once, at the initial full launch.
`build_sdk` still runs on this path (before the `ENGINE_ONLY` early-exit), so if
the applied change bumped a dependency, `ensure_npm_deps` would want to reinstall
the shared workspace `node_modules` — which it must NOT do under a live frontend
(corrupts Vite) and must NOT hard-fail over either (that abort left the workspace
with no engine at all — the "workspace didn't come up after restart" bug). Under
`ENGINE_ONLY` it instead **skips the install, warns, and returns 0** so the engine
comes up on the existing (working) `node_modules`; the stamp is left un-updated, so
the deferred deps install on the next *full* restart (stop + `web-dev.sh`).
Implementation: `start_frontend_built` / `start_frontend_dev` in
`scripts/lib/workspace.sh` (checkout-level build-watch pid in
`crates/lucidos-app/.build-watch/pid`, reused across workspaces and torn down
ref-counted by `cleanup_processes` and `stop.sh` via
`teardown_shared_build_watch_if_idle`). The e2e harness (`scripts/lib/e2e.sh`)
drives `start_vite` without `parse_dev_args`, so it never sets `BUILT` and stays on
the live dev server.

## Engine tests need Postgres — use `test-engine.sh`

The engine's integration tests (`setup_test_db` in `crates/lucidos-engine/src/test_support.rs`) need a **real Postgres**: each test `CREATE`s a throwaway `lucidos_test_*` database, runs migrations, and drops it. The connection comes from `TEST_DATABASE_URL`, falling back to a hardcoded `localhost:5432`. **Running bare `cargo test -p lucidos-engine` with no `TEST_DATABASE_URL` and no PG up makes every DB-backed test panic on connect** (`.expect("admin connect")`) — that's hundreds of false "failures", not regressions.

```bash
make test                       # → ./scripts/test-engine.sh  (cargo test --lib)
make test-full                  # → ./scripts/test-engine.sh --full  (whole crate)
./scripts/test-engine.sh -- -- migration_tests   # pass filters through to cargo test
./scripts/test-engine.sh --fresh                 # recreate the test DB container clean
```

`test-engine.sh` provisions a **dedicated, disposable** `lucidos-pg-test` container (`pgvector/pgvector:pg17`, port `LUCIDOS_TEST_PG_PORT` / default `5510`), exports `TEST_DATABASE_URL`, then runs cargo test. It is isolated from every workspace's PG (separate name + port) so a test run can't mutate `~/workspaces/*` data, and it **never broad-kills** — it touches only its own container by exact name (the prior `test-engine.sh` was deleted for `pkill -f cognos-engine`). To run cargo directly instead, start the container once and `export TEST_DATABASE_URL` yourself.

Always use `web-dev.sh -b` to restart. `scripts/lib/ports.sh` allocates per-workspace ports; engine reverse-proxies to Vite. Postgres containers (`lucidos-pg-<cksum>`) stay running when engine stops.

### macOS code signing (stable TCC grants)

A `cargo build` engine binary is `adhoc, linker-signed`; its CDHash changes every rebuild, so macOS TCC (privacy) discards prior permission grants and re-prompts ("lucidos-engine would like to access …") after each rebuild. `build_or_find_engine` (in `scripts/lib/workspace.sh`) re-signs the freshly built binary with a **stable self-signed identity** (`scripts/lib/codesign.sh` → `sign_engine_binary`), giving it a rebuild-stable Designated Requirement so one Allow click persists. Run `./scripts/dev-codesign-setup.sh` **once** first — it creates + trusts the cert (single GUI password prompt). Until then signing is a no-op and the build proceeds unsigned (with a hint). This only stops the re-prompting; the prompt still names "lucidos-engine" (a post-fork TCC responsibility disclaim to attribute it to Claude Code is not possible — see the note in `runtime/claude_code.rs::build_command`).

**Search-list registration is load-bearing.** `codesign --sign <name>` resolves the identity through the **keychain search list**, not the `--keychain` flag — so the dedicated `lucidos-dev-signing.keychain-db` must be *in the search list* or every sign fails with "no identity found" and silently falls back to ad-hoc (the prompts never stop, even though `find-identity -p codesigning "$KEYCHAIN"` reports the identity as valid). `lucidos_ensure_keychain_in_search_list` (in `codesign.sh`) registers it; both setup and `sign_engine_binary` call it, so existing installs self-heal on the next `-b` build. **Per-binary, not per-workspace:** every engine binary signed with the same identifier (`lucidos-engine`) + same cert leaf shares one Designated Requirement, so a single Allow covers all workspaces. But a binary built outside the scripts — e.g. `cargo run` from an IDE — bypasses `sign_engine_binary` and stays ad-hoc; launch via `web-dev.sh` so it gets signed.

## Build

```bash
cargo build -p lucidos-engine --release    # Engine
cd crates/lucidos-app && cargo tauri build # Desktop app
```

Dev: native engine + Docker PostgreSQL. Production: single Docker container. Makefile: `make build`, `make test`, `make run`.
