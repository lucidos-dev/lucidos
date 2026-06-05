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
it (`LUCIDOS_DEV_PROXY`). The navigation shell (`index.html`) **and** content-hashed
`/assets/*` are cached cache-first by the service worker → instant iOS PWA resume /
notification-tap reload (no ~10s cold-load black screen over Tailscale; the reload
boots with zero network on the critical path, only data GETs round-trip), and the
client only changes when rebuilt, so it can't drift ahead of the engine binary. Trade-off: **no HMR** — after a change is applied, `vite build --watch`
rebuilds (a few seconds), the SW detects the new build (each build stamps a fresh
`BUILD_ID` into `sw.js` via the `lucidos-sw-stamp` plugin in
`crates/lucidos-app/vite.config.ts`), and the existing **"New version available →
Refresh"** toast tells you when to reload.

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
touches the frontend. `kill_stale_processes` skips both the preview kill and the
`vite build --watch` kill when `ENGINE_ONLY` is set, so the already-running built
frontend survives the restart and the new engine just re-attaches its proxy; the
build-watch picks up the merged source and rebuilds `dist/` on its own. The
frontend mode is therefore chosen once, at the initial full launch.
Implementation: `start_frontend_built` / `start_frontend_dev` in
`scripts/lib/workspace.sh` (built-watch pid in `.lucidos/build-watch.pid`, torn
down by `cleanup_processes` and `stop.sh`). The e2e harness (`scripts/lib/e2e.sh`)
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
