# 0016 — Packaged Tauri e2e is a boot smoke test, not UI automation

- **Status** — Accepted
- **Date** — 2026-06-24

## Context

The packaged desktop build (macOS `.app`/`.dmg`, ADR 0012/0014) is a two-process
model: a launchd **gateway** service with **embedded Postgres** plus a GUI
**client** whose **WKWebView** points at the gateway. The existing e2e suite
(`scripts/e2e.sh`: browser / API / wasm / embedder) exercises only the **dev
topology** (a direct engine + Docker Postgres). Nothing covered the *packaged*
topology — the parts that are unique to shipping: staged Resources, the bundled
gateway + engine binaries, relocatable embedded Postgres provisioning, the
per-workspace database, the engine spawn, and static serving through the gateway
proxy. We wanted e2e coverage of the packaged build.

The obvious shape — "drive the packaged app's window like the Playwright browser
suite does" — runs into a hard platform wall.

## Decision

Packaged e2e is a **headless boot smoke test** of the service → gateway →
embedded Postgres → engine → static-serving chain, **not** UI automation of the
WKWebView. It is implemented as `scripts/e2e-packaged.sh`, which runs the
bundle's own service role (`Lucidos --service`) under an isolated temp `HOME` and
asserts the chain over HTTP + on disk. Native (non-UI) logic in the Tauri layer
is covered separately by pure-function unit tests in `crates/lucidos-app`
(`lib.rs`, `notifications.rs`, alongside the existing `desktop.rs` tests).

## Rationale

- **macOS WKWebView has no WebDriver.** Apple's `WKWebView` exposes no WebDriver
  interface, and `tauri-driver` (Tauri's WebDriver bridge) supports only Linux
  (WebKitGTK) and Windows (Edge). There is no supported way to drive the actual
  packaged window in CI on macOS — the platform Lucidos ships first.
- **The boot chain is where packaging actually breaks.** Resource staging, the
  embedded Postgres relocatable tree, the gateway env wiring, and the engine
  spawn are the things that differ from dev and that historically regress. A boot
  smoke test catches exactly those, with no browser in the loop.
- **The service role is cleanly headless.** `Lucidos --service` runs
  `desktop::run_service()` → `spawn_gateway()` and never touches
  AppKit/Tauri/notifications/updater/tray/launchd (those are client-role only),
  so it is fully scriptable with no display and no launchd pollution. A temp
  `HOME` isolates the embedded cluster + workspaces + logs from any real install.

## Consequences

- We get high-signal coverage that the packaged build *boots and serves* — the
  load-bearing assertion is that a freshly created workspace reaches `healthy`
  through the gateway and its engine answers, proving embedded Postgres
  provisioned, the per-workspace DB was created, and the bundled engine spawned.
- We do **not** get coverage of in-window UI behavior in the packaged shell
  (panel webviews, native menus, tray, notification taps). Those remain covered
  indirectly: the same frontend is exercised by the browser suite against dev,
  and the native Rust logic is unit-tested.
- The smoke test is **heavy** (full release + DMG build + a Postgres download),
  so it is a standalone script, kept out of the default `e2e.sh` run; the nightly
  opts in via `e2e.sh --packaged` / `LUCIDOS_E2E_PACKAGED=1`.
- It is **macOS-only** and skips gracefully elsewhere.

## Alternatives considered

- **Drive the packaged window with `tauri-driver` / WebDriver.** Rejected: not
  supported for macOS WKWebView (Linux/Windows only). It cannot run on the
  platform we ship.
- **Point Playwright at the packaged gateway topology** (boot the embedded stack,
  then run the existing browser suite against `https://localhost:<port>/<slug>/`).
  Considered and not chosen for this round: it validates packaging/gateway/
  embedded-PG wiring but drives a *regular browser*, not the WKWebView, so it
  isn't really "the Tauri build" — and it duplicates the dev browser suite's UI
  assertions over a heavier topology. Left as a possible future addition.
- **`tauri::test` MockRuntime tests for the command handlers.** Rejected for the
  native-test half: it adds a `test` feature to the `tauri` dep and a brittle mock
  runtime for commands that mostly need real webviews/windows, for marginal gain
  over testing the extracted pure logic. The repo's established pattern
  (`desktop.rs`) is to extract pure decision functions and unit-test those, which
  keeps `cargo test -p lucidos-app` fast and dependency-light.
