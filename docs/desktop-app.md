# Desktop app (.dmg) — build & release runbook

The macOS desktop app is a **self-contained** Tauri bundle: it ships PostgreSQL +
pgvector, the standalone `lucidos-gateway` binary, the `lucidos-engine` binary,
the JS SDK, and the built frontend inside the `.app`, so an end user
double-clicks the `.dmg`, drags Lucidos to Applications, and launches — no
terminal, no Docker, no dev tools. It auto-updates from GitHub Releases.

Architecture and the *why* behind each choice: [ADR 0012](adr/0012-self-contained-desktop-app.md),
refined by [ADR 0014](adr/0014-multi-workspace-redesign.md).

## How it runs (packaged)

The workspace gateway, its spawned engines, and embedded Postgres are a
**persistent background service**; the window is a client you open and close
(see § Always-on service + mobile access below for the *why*). The bundled Tauri
binary plays two roles, both in
`crates/lucidos-app/src/desktop.rs`:

**Service** (`Lucidos --service`, run by a launchd LaunchAgent — headless, no
window). On boot it:

1. resolves the OS app-data dir (survives updates) and the bundle `Resources`
   dir,
2. spawns the bundled `lucidos-gateway` on the **stable** port (default `5252`)
   with `LUCIDOS_GATEWAY_DATA`, `LUCIDOS_GATEWAY_PG_BACKEND=embedded`,
   `LUCIDOS_PG_BIN_DIR`, `LUCIDOS_PG_LIB_DIR`, `LUCIDOS_ENGINE_BIN`,
   `LUCIDOS_STATIC_DIR`, `LUCIDOS_SDK_DIR`, `FASTEMBED_CACHE_DIR`, and
   `LUCIDOS_FALLBACK_MOCK=1`,
3. the gateway creates/loads the workspace registry, auto-creates the first
   `default` workspace, provisions embedded Postgres for workspaces that need it,
   and spawns one loopback-only `lucidos-engine` per running workspace,
4. supervises the gateway; the gateway supervises workspace engines and can
   re-adopt already-running engines after a gateway restart. On explicit
   `launchctl bootout` ("Quit Lucidos"), the service tears down the gateway and
   every engine it spawned.

**Client** (the GUI app the user double-clicks). On launch it:

1. installs/updates `~/Library/LaunchAgents/com.lucidos.engine.plist` and
   bootstraps the service if it isn't already loaded (it starts at login via
   `RunAtLoad`),
2. waits for the gateway health endpoint (`/~/api/v1/health`) on the stable
   port, then points the window at `http://localhost:<port>` (smart root: one
   workspace opens directly; multiple workspaces show the picker).

Closing the window leaves the service running (triggers, scheduled tasks,
coding-agent sessions, and mobile push keep going headless). Only the explicit
**Quit Lucidos** menu item (Cmd+Q) stops the service (`launchctl bootout`).

The stable gateway port is persisted at `<app-data>/config/engine-port`
(historical file/env name; default `5252`; override with `LUCIDOS_ENGINE_PORT`)
so the mobile connect URL never changes across restarts. The gateway is the
network-facing surface; packaged engines bind loopback-only behind it.

`LUCIDOS_FALLBACK_MOCK` lets workspace engines boot before any provider is
configured (they would otherwise panic). First run is mock; the user adds a
provider in **Settings → Providers**, then restarts into the real provider.

None of this runs in development — `scripts/tauri-dev.sh` still uses Docker
Postgres + a natively-built engine, and the client launcher/updater
short-circuit on `tauri::is_dev()` (the service role is only ever started by
launchd in a packaged build).

## Building locally (unsigned)

```bash
cargo install tauri-cli --locked   # one-time
./scripts/build-dmg.sh
```

This builds the frontend, the release gateway + engine, fetches the relocatable
PostgreSQL 18 and compiles pgvector against it (the proven
`scripts/prototype/desktop-pg-pgvector-spike.sh` recipe), stages everything into
`crates/lucidos-app/bundle-resources/`, and runs `cargo tauri build --bundles
app,dmg`. The result is an **unsigned** `.dmg` under `target/release/bundle/dmg/`
— Gatekeeper blocks it on other Macs (right-click → Open to run locally).

The lightweight packaging contract check runs without a macOS bundle build:

```bash
./scripts/build-dmg.sh --check
```

## Shipping (signed, notarized, auto-updating) — credentialed steps

These need an Apple Developer account, a Tauri signing key, and GitHub Releases.
They can't run from a CC worktree; do them on a Mac with the secrets present.

> **Automated by `scripts/release.sh` (host arch).** Cutting a release now builds
> the signed + notarized `.dmg` and uploads it — plus the auto-update artifacts
> (`.app.tar.gz`, `.app.tar.gz.sig`, and a generated `latest.json`) — to the
> GitHub Release it creates. The release **refuses to start** (before any
> force-push) unless all of `APPLE_SIGNING_IDENTITY`, `APPLE_ID`,
> `APPLE_PASSWORD`, `APPLE_TEAM_ID`, and `TAURI_SIGNING_PRIVATE_KEY` are set and
> `plugins.updater.pubkey` in `tauri.conf.json` is a real key (not the
> placeholder) — see `scripts/lib/release_signing.sh`. So the steps below are now
> mostly *setup* you do once (cert, updater key, pubkey); a release run performs
> the build + upload for you. The bundle version is stamped from the `RELEASE`
> file so artifacts are named `Lucidos_<version>_<arch>`. Coverage is the
> **host architecture only** — an Intel-only or Apple-Silicon-only `latest.json`;
> dual-arch is the CI follow-up below.

### 1. Apple Developer ID + notarization

Get a *Developer ID Application* certificate (Apple Developer Program), then:

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID="you@example.com"
export APPLE_PASSWORD="app-specific-password"   # appleid.apple.com → App-Specific Passwords
export APPLE_TEAM_ID="TEAMID"
./scripts/build-dmg.sh
```

`build-dmg.sh` explicitly signs and verifies the bundled `lucidos-gateway` and
`lucidos-engine` resource binaries, codesigns the `.app` with a hardened runtime
(`--deep` also signs the nested Postgres binaries/libs), refreshes and signs the
DMG payload, submits the `.dmg` to `notarytool`, and staples the ticket. Without
notarization, Gatekeeper blocks the download.

### 2. Tauri updater signing key

```bash
cargo tauri signer generate -w ~/.tauri/lucidos-updater.key
```

Put the printed **public** key in `crates/lucidos-app/tauri.conf.json` →
`plugins.updater.pubkey` (replace `REPLACE_WITH_TAURI_SIGNER_PUBLIC_KEY`). At
build time, export the private key so the bundler signs the update artifacts:

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/lucidos-updater.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="…"
```

This emits `*.app.tar.gz` + `*.app.tar.gz.sig` alongside the `.dmg`.
`bundle.createUpdaterArtifacts: true` is already set in `tauri.conf.json` (Tauri
v2 requires it for the macOS updater tarball), so you don't need to add it.

### 3. GitHub Releases + `latest.json`

`scripts/release.sh` does this automatically: it creates the Release tagged
`v<version>` on `github.com/lucidos-dev/lucidos` and uploads the `.dmg` (first
install), the `.app.tar.gz`, its `.sig`, and a generated `latest.json` (asset
uploads use `--clobber`, so re-running a release replaces them). The generated
manifest looks like this — `signature` is the verbatim contents of the
`.app.tar.gz.sig`, the single platform key is the host triple:

```json
{
  "version": "0.10.0",
  "notes": "What changed.",
  "pub_date": "2026-06-16T00:00:00Z",
  "platforms": {
    "darwin-aarch64": { "signature": "<contents of .app.tar.gz.sig>", "url": "https://github.com/lucidos-dev/lucidos/releases/download/v0.10.0/Lucidos.app.tar.gz" },
    "darwin-x86_64":  { "signature": "<contents of .app.tar.gz.sig>", "url": "https://github.com/lucidos-dev/lucidos/releases/download/v0.10.0/Lucidos.app.tar.gz" }
  }
}
```

`plugins.updater.endpoints` already points at
`…/releases/latest/download/latest.json`, so a published Release is picked up on
the next launch: the app prompts *"Update available — restart now?"*, then
downloads, installs, and relaunches.

### 4. CI

There is no CI yet. A `tag → build → sign → notarize → publish` GitHub Actions
workflow (macOS runner, the secrets above) is the natural home for steps 1–3 so
releases are reproducible. Build the `x86_64` and `aarch64` bundles for full
coverage (`TARGET_TRIPLE` selects the relocatable PG; pass the matching Rust
target to the engine build).

## Always-on service + mobile access (implemented 2026-06-16)

> **Implemented** in `crates/lucidos-app/src/desktop.rs` (the service +
> LaunchAgent + stable-port lifecycle) and `crates/lucidos-app/src/mobile.rs`
> (connect URLs + Tailscale setup, surfaced in **Settings → Mobile Access**).
> This **supersedes** the window-coupled lifecycle the initial foundation
> shipped (`desktop.rs` used to boot the stack on launch and tear it down on
> `RunEvent::Exit` / `restart_app`). The runtime can only be fully verified by
> building a real `.app` on a Mac (launchd `bootstrap`/`kickstart`/`bootout`,
> the `--service` role, the `tailscale` CLI calls); the code `cargo check`s and
> the frontend type-checks + unit-tests clean.

**The gateway service is persistent; the UI is a client you open and close.**
Closing the window must NOT stop the gateway or workspace engines — triggers,
scheduled tasks, coding-agent sessions, and mobile push all have to keep running
with no window open (this is Lucidos's always-on event model; see CLAUDE.md §
Engine Statelessness).

- **Run the gateway service as a macOS launchd LaunchAgent.** The `.app` installs
  a plist into `~/Library/LaunchAgents/com.lucidos.engine.plist` on first run
  with `RunAtLoad` + `KeepAlive` (start at login, restart on crash, headless).
  The Tauri window and the mobile PWA are both pure clients of it. Teardown
  happens ONLY for an explicit "Quit Lucidos", not for window close.
- **Stable gateway port, not a random one.** The connect URL is stable across
  restarts. The packaged gateway owns the network-facing port; engines bind
  loopback-only behind it.
- **Show the connect URLs.** Surface localhost / LAN / Tailscale URLs (like the
  dev `show_banner`) so the user knows what to open on the phone.

**Mobile access = Tailscale (chosen), with the auto-setup reality:**

- **Mac side (scriptable after consent):** detect `tailscale`; if missing, guide
  the install (system VPN — needs user consent, can't be silent); run
  `tailscale up` (one-time tailnet login, or an auth key) then
  `tailscale serve https / http://127.0.0.1:<port>` for an auto-renewed HTTPS
  cert at `https://<machine>.<tailnet>.ts.net`. Full PWA + push, works off-LAN.
- **Phone side (guided, not silent):** OS sandboxing prevents remote install/login
  — show a QR/link to install Tailscale and join the **same tailnet** (auth key
  can pre-authorize). Then open the `…ts.net` URL.
- **Use `serve`, not `funnel`.** The engine has **no inbound API auth**, so keep
  it tailnet-private (`serve`); do not expose it publicly (`funnel`) without first
  adding an inbound auth token. This is also why Tailscale is preferred over
  binding the raw LAN.
- **Fallbacks** when Tailscale isn't wanted: the mkcert local-CA route (LAN-only,
  README documents iOS trust) for PWA/push, or plain HTTP on LAN (browser only —
  no service worker / push, and unauthenticated LAN exposure).

## Status / remaining

The buildable foundation (launcher, standalone gateway handoff, engine
static-serving + mock fallback, updater wiring, bundle config, `build-dmg.sh`)
is in the tree and compiles. The signed build + `latest.json` + Release upload
are now wired into `scripts/release.sh` (host arch). What remains is
credentialed/Mac-only and tracked separately: the one-time updater keypair +
real `pubkey`, a first real notarized bundle build, dual-arch coverage, CI, and a
clean-machine first-run check.

Known follow-ups for the packaged build (surface them in the clean-machine pass):

- **Gateway-first packaged boot — wired (verify on a clean machine).**
  `build-dmg.sh` stages `lucidos-gateway` next to `lucidos-engine`, the launcher
  starts the gateway, and the gateway spawns engines by `LUCIDOS_ENGINE_BIN`.
  Confirm first-run `default` workspace creation and picker/open behavior in a
  signed `.app`.
- **In-bundle JS SDK — wired (verify at runtime).** `/api/v1/sdk.js` (used by
  app-UI iframes) is now staged: `build-dmg.sh` copies `packages/lucidos-sdk/dist`
  to `<resources>/sdk`, the launcher sets `LUCIDOS_SDK_DIR`, and the engine's
  `find_sdk_bundle` checks it first. Confirm app UIs load the real SDK (not the
  warning stub) in the packaged build.
- **The primary "Restart" control** *(implemented)*. A workspace engine reports
  `packaged: true` from proxied `/api/v1/health`, and the frontend routes the
  Restart control accordingly: packaged + Tauri → `restart_service` (Tauri runs
  `launchctl kickstart -k` on the LaunchAgent), packaged browser/PWA →
  `POST /api/v1/restart` (the engine asks the gateway control API to respawn
  that workspace stack, with launchd as the legacy fallback), dev →
  `POST /api/v1/restart` (spawns `web-dev.sh --engine-only`). The supervisor
  catches SIGTERM and tears the gateway stack down gracefully before launchd
  respawns it. (The Tauri **"Restart App"** action restarts only the GUI client
  now — the gateway is the launchd service.)
