# E2E Test Design Decisions

Decisions and tradeoffs made while building the end-to-end test suite.

## Architecture: Layers

1. **Browser E2E tests** (Playwright, `crates/lucidos-app/e2e/`) — drive a real browser against the running Lucidos UI. Three projects run by default: `chromium` (desktop), `mobile` (mobile Chromium), and `mobile-webkit` (iOS Safari emulation via WebKit).
2. **HTTP API tests** (Rust, `crates/lucidos-e2e/tests/api_support/`, workspace member crate `lucidos-e2e`) — hit the API directly without a browser
3. **Packaged build smoke test** (`scripts/e2e-packaged.sh`) — boots the macOS `.app` itself (service role + embedded Postgres) and asserts the packaged boot chain over HTTP + on disk. macOS-only, opt-in, does **not** drive the WKWebView (see below).

The browser + API layers require a running dev workspace (`~/workspaces/e2e-test` by default). The packaged smoke test boots the bundle's own isolated stack instead.

## Key Decisions

### Packaged build e2e is a boot smoke test, not UI automation
The packaged build (macOS `.app`/`.dmg`) is a launchd **gateway** service with **embedded Postgres** plus a GUI **client** whose **WKWebView** points at the gateway. We cannot drive that window in CI: Apple's WKWebView exposes no WebDriver, and `tauri-driver` supports only Linux (WebKitGTK) + Windows. So `scripts/e2e-packaged.sh` is a **headless boot smoke test** of the service → gateway → embedded Postgres → engine → static-serving chain — the parts unique to packaging (staged Resources, the bundled binaries, the relocatable Postgres tree, the per-workspace DB, the engine spawn) that dev e2e (direct engine + Docker PG) never exercises. The load-bearing assertion: a freshly created workspace reaches `healthy` through the gateway and its engine answers, which can only happen if embedded Postgres provisioned, the DB was created, and the bundled engine spawned and served. Full rationale + roads-not-taken: ADR 0016.

### Smoke test runs the bundle's service role under a temp HOME
It launches `Contents/MacOS/Lucidos --service` (`desktop::run_service` → `spawn_gateway`), which never touches AppKit/Tauri/notifications/updater/tray/launchd — those are client-role only — so it is cleanly headless and scriptable, with no display and no launchd pollution. `app_data_dir_from_env()` derives the data dir from `$HOME`, so the test runs under a temp `HOME`: the embedded cluster + workspaces + logs are fully isolated from any real install and removed on teardown. A free ephemeral port (clear of dev 5251 / packaged 5252) is passed via `LUCIDOS_ENGINE_PORT`, and the dev workspace's `DATABASE_URL`/`PG*`/`LUCIDOS_*` env is scrubbed at launch so inherited CC-session values can't poison the packaged embedded path.

### fastembed cache is seeded, not re-downloaded
`spawn_gateway` pins `FASTEMBED_CACHE_DIR` to `<app-data>/fastembed`; under a temp `HOME` that is cold (~465 MB HF download + networked warmup at engine boot). The smoke test symlinks that dir to the machine-persistent shared cache the embedder e2e already seeds (`${XDG_CACHE_HOME:-$HOME/.cache}/lucidos/fastembed`), so a seeded host is offline/fast and a cold host downloads the model once (warming future runs).

### Packaged smoke test is standalone + opt-in (heavy build)
Building the `.app` is a full release engine+gateway build + a relocatable Postgres download + a frontend build + `cargo tauri build`. Too heavy for every run, so `e2e-packaged.sh` is a standalone script kept out of the default `e2e.sh`; the nightly opts in via `e2e.sh --packaged` / `LUCIDOS_E2E_PACKAGED=1`. It is macOS-only and skips gracefully elsewhere.

### Native (Tauri) non-UI logic is unit-tested via pure-function extraction
The Tauri command layer (`lib.rs`, `notifications.rs`) is mostly GUI/IPC glue that needs real webviews/windows, which a unit test can't supply. Following the established `desktop.rs` pattern, the load-bearing *decision* logic is extracted into named pure functions and unit-tested with no Tauri runtime: `safari_ua`/`heartbeat_expired`/`is_app_window` (`lib.rs`) and `link_identifier`/`is_dismiss_action` (`notifications.rs`, the notification tap-routing + replace-by-id semantics). **Rejected:** `tauri::test` MockRuntime tests — they add a `test` feature to the `tauri` dep and a brittle mock runtime for marginal gain, and keep `cargo test -p lucidos-app` slower/heavier. Note `cargo test -p lucidos-app` needs a built `crates/lucidos-app/dist/` for `generate_context!` to compile, so a frontend build must run first.

### Dual layout handling
Lucidos renders desktop and mobile layouts simultaneously — every DOM element exists twice. All Playwright selectors use `.first()` and visibility checks via `getBoundingClientRect()` to target only the visible (desktop) element. The `openThreadDrawer()` helper uses `page.evaluate()` with rect checks rather than Playwright's `.isVisible()` to avoid false positives from the hidden mobile layout.

### Thread drawer is collapsed by default
At 1280x800 viewport, the thread drawer starts collapsed. Tests that need it must call `openThreadDrawer()` which clicks the toggle button if the drawer isn't already visible.

### Reload tests don't rely on auto-focus
After `page.reload()`, localStorage-based thread auto-focus is unreliable in headless Chromium (timing issues with SSE reconnection and thread data loading). Reload tests instead:
1. Record the thread ID before reload
2. After reload, open the thread drawer
3. Explicitly click the thread to re-focus it
4. Then verify messages/state persisted

This tests the important thing (data persistence) without depending on the auto-focus race condition.

### Self-signed TLS
Lucidos uses HTTPS even in dev (Vite TLS). Both Playwright (`ignoreHTTPSErrors: true`) and Rust tests (`danger_accept_invalid_certs`) accept self-signed certificates.

### Port discovery
Both test layers read the workspace ports from `<workspace>/.lucidos/ports`. The workspace path is configurable via `E2E_WORKSPACE` environment variable, defaulting to `~/workspaces/e2e-test`.

### Unknown API routes return SPA fallback
The engine proxies unknown `/api/v1/*` routes to Vite, which returns the SPA HTML fallback with status 200. The Rust error test verifies the response is not valid JSON (i.e., it's HTML) rather than asserting a specific HTTP status code.

### Unique message markers
Every test uses `uniqueMessage(prefix)` to generate collision-free messages with timestamps and random suffixes. This prevents test interference when running against a shared workspace with existing data.

### LLM-dependent tests
Several tests send messages to the LLM and assert on responses. These tests use generous timeouts (90s for response completion) and assert on structural properties (non-empty content, visible element) rather than exact text content, since LLM output is non-deterministic.

### SSE streaming test
The streaming test captures text at two points during response generation. It asserts both snapshots are truthy (content appeared) rather than asserting the second is longer than the first, which is flaky with fast models that complete before the 1.5s delay.

### Rust API test module structure
Rust's module system doesn't allow both `tests/api.rs` and `tests/api/v1/mod.rs`. The solution uses `#[path]` attributes in `tests/api.rs` (in the `lucidos-e2e` crate) to include submodules from a `tests/api_support/` directory.

### Separate `lucidos-e2e` crate
API tests live in their own workspace member crate, not in `lucidos-engine`'s `tests/`. This keeps `cargo test -p lucidos-engine` from compiling them (so it stays fast and infra-free) and removes the need for `#[ignore]` on tests that require a running workspace. Run via `./scripts/e2e-api.sh` or the umbrella `./scripts/e2e.sh`.

### Single-writer lock on the e2e workspace
Every e2e entry point (`e2e.sh`, `e2e-browser.sh`, `e2e-api.sh`) acquires `~/workspaces/e2e-test/.lucidos/e2e.lock` (PID + `$LUCIDOS_THREAD_ID` + worktree path + start time) before starting the workspace or any browser. A second invocation while the lock is held (owner PID alive) exits 1 with a message naming the holder. The lock exists because two CC sessions running Playwright concurrently against the shared workspace race on browser processes — on 2026-04-19 a WebKit GPU child leaked to 28 GB and OOM-rebooted a 32 GB Mac.

**Reclaiming a stale lock is orphan-safe, not blind.** A "stale" lock is one whose owner PID is dead — but an *interrupted* run (killed before its EXIT trap could tear down) leaves orphaned e2e processes alive: Playwright/WebKit browser children and the e2e-test workspace engine, still holding RSS. The old reclaim treated "owner dead" as "safe to start fresh", so on 2026-06-21 the nightly orchestrator re-spawned the full suite three times and each re-spawn reclaimed the free stale lock and stacked a fresh set of browsers on top of the orphans → 23.5 GB compressed + 14 GB swap, machine pinned in critical memory pressure for 4+ hours. `acquire_e2e_lock` now scans for those orphans before reclaiming (browser children matched by the `ms-playwright/*` cache path — same discriminator the webkit reaper uses; the engine via its own `engine.pid`), runs a **deliberate, logged sweep** (SIGKILL the browsers, SIGUSR1 the engine so its supervisor stops cleanly), re-scans, and reclaims only once they are gone. If the sweep can't clear them it **refuses** with an actionable error rather than stack. The four states: no lock → acquire; live-PID lock → hard-fail; stale + no orphans → reclaim; stale + orphans → sweep then reclaim, else refuse. (Deliberately *not* swept: any `vite`/web-dev server — under ADR 0014 e2e runs a one-shot `vite build`, no long-lived server, and a name-based match would risk SIGKILLing the checkout-level shared build-watch that serves other workspaces.) Lock logic in `scripts/lib/e2e_lock.sh`; covered by `scripts/lib/e2e_lock_test.sh` (run directly, no harness — hermetic, fakes orphans with sleepers, never spawns a browser).

### Worktree cleanup must never delete branches by name or ancestry
`cleanup_e2e_worktrees` (in `scripts/lib/e2e.sh`, run on browser-test teardown) removes the worktrees that e2e-spawned CC sessions leave behind, so the canonical lucidos repo doesn't accumulate dead worktrees that blow the engine's 30s startup-recovery budget. The trap: those worktrees register in the **canonical** `~/projects/lucidos` repo — the same repo every *real* CC session (dev/personal worktrees + their `claude-code/*` branches) lives in — and `$_E2E_PROJECT_DIR` is whichever checkout invoked the script, frequently a real CC worktree of that repo. An earlier version deleted every `claude-code/*` branch whose tip was an ancestor of `main` ("no unique work"), assuming real sessions are always ahead of main. That's false for a **just-started** session that hasn't committed yet: its branch sits exactly at main. On 2026-06-13 this force-deleted a live session's branch and wiped its worktree mid-task. The only safe discriminator for "created by an e2e run" is the **worktree path under `$E2E_WORKSPACE`**; cleanup now removes only those worktrees and deletes only the branch each was checked out on. Regression: `scripts/lib/e2e_test.sh::test_cleanup_spares_real_cc_sessions` (run directly, no harness).

### mobile-webkit navigation wedge — system-proxy (PAC/WPAD) discovery (fixed at source) + a residual cold-start stall
**Symptom.** On the `mobile-webkit` project, a navigation hangs: the FIRST
app-root navigation in a fresh context times out (30 s), the test never reaches
its app-specific readiness gate, and a (random) test fails — observed victims rotate over runs
(drawer-divider-thickness, sdk-confirm, drafts, threads, message-route-panel,
file-search, thread-search, and most recently `file-edit.spec.ts:55` +
`streaming.spec.ts:49`). It passes on Playwright's fresh-context retry. The flake
is WebKit-only.

**Ruled out (2026-06 investigation).** The wedge is NOT the dev server, engine,
reverse-proxy, transport, or product code:

- Serving the **built**, bundled, content-hashed `dist` via `vite preview` still
  wedges → not the dev server's unbundled module graph.
- Forcing **HTTP/1.1** (ALPN h1-only) still wedges, and so does **HTTP/2** → not
  a protocol issue.
- Connecting over **IPv4** (`127.0.0.1`) still wedges, and so does **IPv6**
  (`::1`) → not the dual-stack/loopback IP family.
- Over **plain HTTP** the failure moves to `browserContext.newPage()` hanging —
  the browser-side network-session setup, NOT the engine.
- Instrumenting the engine's reverse-proxy + request entry showed that during a
  wedge the engine is healthy (serves other requests in ~1 ms) and the `/`
  request **never reaches it** (a multi-second gap with zero inbound
  navigations). The proxy's upstream hop to Vite never stalls or errors.

The first version of this note concluded from the above that the WebKit browser
process simply "freezes" under generic system contention and that *no* config
change could prevent it — only `gotoWithRetry` + `retries: 1` could recover it.
That conclusion was **incomplete**: it identified *where* (browser-side, before
the request leaves the box) but never tested the proxy lever, which prevents the
dominant cause outright.

**Root cause — TWO distinct variants (both browser-side, both first-nav).**

*Variant 1 (PRIMARY, deterministic, now fixed): macOS system proxy / PAC (WPAD)
auto-discovery.* WebKit's network process runs system proxy discovery
synchronously when it initialises a fresh network session — and Playwright
creates a fresh context (= fresh network session) per test. On a **managed/MDM
Mac** (an MDM-managed corporate fleet) the system network
config can carry "Auto Proxy Discovery" or a PAC URL pushed by a config profile,
active on the corp network. The discovery does a DNS lookup for `wpad`, a
captive-portal probe, and/or a PAC fetch; under contention that round trip stalls
for tens of seconds, then self-clears. This explains every observation above:
the `/` request queues *behind* proxy resolution so it never reaches the engine;
a fresh context re-resolves (often now cached/fast) and recovers; Chromium is
immune because Playwright launches it without the system proxy; `newPage()`
itself can hang because that is when the network session initialises; and
protocol / IP-family / dev-vs-dist are all irrelevant because proxy resolution
precedes them. This is the variant the [Playwright community](https://playwright.dev/docs/network)
and multiple upstream issues name as the #1 cause of slow first-`goto`/`newPage`
on WebKit/macOS.

*Variant 2 (RESIDUAL, intermittent, recovered not prevented): WebContent
cold-start / document-load stall under heavy host contention.* This one occurs
**even with no system proxy configured** — directly observed: a clean full
`mobile-webkit` run on a dev host whose `scutil --proxy` is empty still wedged
once on its 220th-ish test (the old DCL-gated `page.goto('/')` timed out at 30 s, recovered on
the fresh-context retry), at the run's peak contention (after ~220 tests + dozens
of `claude` subprocess spawns + 20 worktrees). An `about:blank` warmup was tried
to pre-spawn the render process off the timeout-critical clock and did **not**
prevent it — the stall is in the real document load, which a blank nav doesn't
warm — so the warmup was removed rather than shipped as dead weight. The only
thing that reliably clears variant 2 is a fresh browser context, i.e. the
whole-test `retries: 1` (plus the RSS reaper for the host-memory side).

(The earlier note's claim that "a host with no system proxy does not wedge at
all" is therefore **wrong** — corrected here. No proxy removes variant 1, not
variant 2.)

**Fix (at the source, for variant 1).** `crates/lucidos-app/playwright.config.ts`
sets an explicit `proxy` on the `mobile-webkit` project:

```ts
proxy: { server: 'http://127.0.0.1:1', bypass: 'localhost,127.0.0.1,::1' }
```

Providing *any* explicit proxy makes WebKit skip system auto-discovery entirely.
The e2e suite loads only localhost, so every URL matches `bypass` and connects
**direct** — the inert loopback dead-port `server` is never contacted (and is
fast-refused if it ever were). No WPAD/PAC, no stall, first navigation succeeds on
the first attempt. Deterministic and load-independent. Scoped to `mobile-webkit`
because the flake is WebKit-only; the e2e suite makes no external requests (web
fonts default to system and are non-blocking), so localhost-direct is hermetic
and lossless.

**Safety nets for variant 2 (kept — recovery, not prevention):**

- The mobile-webkit `context` fixture (`e2e/fixtures.ts`) now preflights every
  fresh browser context with a cheap same-origin `/api/v1/health` navigation
  before the test's `page` fixture is created. If that first navigation cannot
  commit promptly, the fixture discards the cold context and creates a clean
  one. This makes "context can reach localhost" a setup invariant instead of
  letting a random spec be the first consumer of a wedged WebKit context.
- `gotoWithRetry` (`e2e/helpers.ts`) waits only for the main-document response
  commit, bounds any pre-commit hang to 2×30 s (vs. the full 120 s test budget),
  and re-navigates once; callers assert real readiness explicitly afterwards.
  No warmup (tried, ineffective — see above).
- The whole-test `retries: 1` stays — a fresh context is what actually clears
  variant 2 (it also absorbs unrelated Chromium context-init flakes).
- The WebKit RSS reaper (below) stays as the host-memory safety net.
- `scripts/e2e-browser.sh` still runs `mobile-webkit` first and phase-splits
  nav/CC specs — that shrinks variant 2's contention window, harmless to keep.

*Spotlight is NOT a usable lever, despite intuition.* The `.metadata_never_index`
marker is **deprecated and a no-op on macOS 26** (verified 2026-06: a file inside
a directory carrying the marker is still returned by `mdfind`, identically to an
unmarked directory). The mechanisms that still work — a `.noindex` dir-name suffix
or dot-prefixing a dir (hidden) — can't apply to `target/`/`dist` without breaking
the build. In practice the e2e build churn already lives under
`<workspace>/.lucidos/…`, a dot-hidden path Spotlight does not index, so it is
already excluded; no marker is needed or effective.

**Verification (2026-06, dev host, `scutil --proxy` empty).**

- The two originally-reported specs — `file-edit.spec.ts:55` + `streaming.spec.ts:49`
  — pass on the FIRST attempt with `--retries=0`, both run in isolation and inside
  the full project. No retry-masking.
- Full `mobile-webkit` project (`--webkit`, default `retries: 1`): 220 passed.
  Variant 2 fired exactly once (`thread-title-edit.spec.ts:203`, `page.goto`
  DCL-timeout at peak contention, recovered on the fresh-context retry) — same
  rate as historically, and the host has no proxy so variant 1 could not have been
  involved.
- Three deterministic failures (`control-menu.spec.ts:97`,
  `message-route-panel.spec.ts:11`, `:55`) are **pre-existing and unrelated** to
  this change: they fail identically on a baseline checkout (changes stashed) with
  `<div id="app"> intercepts pointer events`, i.e. the overlay inert-behind feature
  (`c86c2adec`) made an anchor/backdrop target `pointer-events: none` while the
  popover is open and these specs `.click()` it without `force: true`. Separate
  subsystem; tracked separately, not fixed here.

**Follow-up verification (2026-06-16, flaky-recovered nightly).**

- Nightly recovered three first-attempt failures: `mobile-webkit`
  `paste-link-substitution.spec.ts:124`, `mobile-webkit` `drafts.spec.ts:65`,
  and `chromium` `coding-agent-stuck-waiting.spec.ts:98`.
- Focused repeats with `--retries=0` did not reproduce the three reported tests,
  but a full `mobile-webkit --retries=0` run did reproduce the residual as
  pre-commit `page.goto('/')` timeouts in unrelated random victims
  (`drafts.spec.ts:436`, then `repo-files.spec.ts:218`). That proved the old
  DCL-gated helper was only part of the issue: the remaining wedge can happen
  before the main response commits.
- After adding the mobile-webkit context preflight, full `mobile-webkit
  --retries=0` passed 228/228. Final-source focused reruns also passed the
  reported/victim set (`paste-link-substitution.spec.ts:124`,
  `drafts.spec.ts:65`, `drafts.spec.ts:436`, `repo-files.spec.ts:218`) and the
  mobile navigation suite with retries disabled.
- The chromium recovered test was independent: its nightly 0 ms failure shape
  matched setup/worker noise, and it passed focused, repeated, and inside full
  chromium with `--retries=0`. A separate full-chromium sync failure in
  `settings-backup-navigation-desktop.spec.ts` was fixed by waiting for the
  page's SSE stream before emitting the transient `/api/v1/ui/navigate` event.

**What this proves and doesn't.** Variant 1's wedge only reproduces where a system
proxy/PAC is configured (the managed nightly host on the corp network), so a local
run proves *no regression* (direct localhost works, no font/proxy breakage,
targets green first-attempt) but cannot trigger — and so cannot independently
prove elimination of — variant 1. To confirm the variant-1 diagnosis on the
nightly host, run `scutil --proxy` there: a non-empty result with
`ProxyAutoDiscoveryEnable = 1` or a `ProxyAutoConfig*` entry is the smoking gun,
and the proxy fix removes that path for WebKit. Variant 2 is intermittent and
load-dependent; treat a falling `webkit_reaps` count and zero flaky-recovered
mobile-webkit specs over several nightlies as the real signal. Do not interpret a
single green local `e2e-browser.sh` as full proof.

#### `drafts.spec.ts:65` — the per-test assertion timeout, not the wedge

`drafts.spec.ts:65` ("thread draft persists when switching to compose and back")
was the longest-running mobile-webkit flake (six sessions of compose-draft clobber
fixes — see `docs/plans/2026-06-27-mobile-webkit-shard-contention.md` and
`docs/plans/2026-06-28-drafts-sse-empty-clear-guard.md`). After all four inbound
draft-clear paths were guarded (`stageDraftFromApi`, `applyRemoteCompose`, the
`MessageReceived` echo — all gated on `hasLocalDraftEdit`), it still surfaced once
on the 2026-06-28 nightly as a **retry-recovered flake** — but with a *different
shape* than the value='' clobber: the restore assertion **timed out** (the textarea
hadn't hydrated within 5s, then recovered on the fresh-context retry).

Root cause of that face: **the test's own assertion timeout, not a product gap.**
The restore step asserted `toHaveValue('thread draft text', { timeout: 5_000 })` —
an explicit **5s**, 6× tighter than the suite's 30s `expect` default. On a re-focus
the draft restore does **zero** network round-trips (`loadThreadEvents` early-returns
when `eventsLoaded`; the draft is already in the local `composeDrafts` signal), and
the sync into the textarea is one render+effect cycle. The only thing that makes it
slow is the documented **WebContent starvation paint freeze** (variant 2 above) —
a slow-**but-correct** restore. The 5s assertion converted that into a failure where
the 30s default would have passed.

Fix (test-only, 2026-06-29): the restore assertion now uses the suite's default
`expect` timeout (no explicit 5s). This is **not** masking — a genuine clobber /
not-stored bug leaves the draft empty *forever*, so it still fails loudly at 30s; the
longer wait only stops a slow-but-correct restore from flaking. The assertion is also
**instrumented**: on failure it queries the persisted draft
(`thread_summaries.compose_text`, written synchronously by the compose PUT) and
classifies the face — `compose_text === the draft` → **CLOBBER** (stored server-side
but absent from the textarea after 30s; a product clear-path bug), `compose_text ===
''` → **NOT-STORED** (the PUT never landed: a `fill()`→`updateCompose` race or a
failed PUT). This ends the multi-session "which face is it?" guessing: a future
occurrence self-diagnoses in the failure message instead of needing a fresh blind
investigation. Do **not** re-tighten this assertion back to a short explicit timeout —
that is the exact change that re-introduces the flake.

#### WebKit RSS reaper — host-resource safety net (distinct from the test-suite self-heal)

The mitigations above (`gotoWithRetry` + `retries: 1`) protect the **test result**:
a wedged navigation fails fast and recovers on a fresh context. They do **not**
protect the **host**. A wedged `com.apple.WebKit.WebContent` process sits on its
RSS without exiting, so under nightly load several pile up and exhaust host
free/compressed memory. On 2026-06-07 that froze a 48 GB Mac (free → ~0.01 GB,
compressed → ~7.5 GB); it has precedent — on 2026-04-19 a WebKit GPU child leaked
to 28 GB and OOM-rebooted a 32 GB Mac (see the e2e.lock note above).

The **WebKit RSS reaper** (`scripts/lib/webkit_reaper.sh`) closes that
host-resource gap. It is a background loop, started by `scripts/e2e-browser.sh`
once the browser run begins, that every few seconds samples the RSS of
Playwright's WebKit child processes and **SIGKILLs any single one over a
configurable cap**. Killing a wedged WebContent child is safe for exactly the
reason the hang is: Playwright's `retries: 1` fresh-context retry recovers the
affected test, identical to the existing self-heal. The reaper is **additive** to
`gotoWithRetry` + `retries: 1`, never a replacement.

It is a **safety net, not a cure**: it does nothing about the wedge itself (still
browser-side, still load-dependent), it only stops a wedged process from taking
the whole machine down with it. If the reaper fires often in the nightly output,
that is the signal that the underlying wedge is getting worse — worth surfacing in
the Concerns rollup.

- **What it matches.** Only processes whose full `ps … command=` path contains the
  Playwright WebKit browsers-cache token (default `ms-playwright/webkit`, or
  `$PLAYWRIGHT_BROWSERS_PATH/webkit` when that env is set). On macOS the
  WebContent/GPU/Networking XPC services all live under
  `…/ms-playwright/webkit-NNNN/`, so this catches the whole WebKit process tree.
  It deliberately does **not** match by a bare `WebContent` substring, so it never
  touches the user's own Safari/Chrome, the `lucidos-engine`, `node`/`vite`,
  Playwright's **chromium** (`ms-playwright/chromium-NNNN/`), or unrelated WebKit
  consumers. PID ≤ 1, the script's own PID, and the reaper's own loop are skipped.
- **Knobs.** `E2E_WEBKIT_RSS_CAP_MB` (default `6144` — well above a healthy
  WebContent, well below the level that exhausts a 48 GB host),
  `E2E_WEBKIT_REAP_INTERVAL_S` (default `5`), `E2E_WEBKIT_REAP_MATCH` (override the
  candidate substring), and `E2E_WEBKIT_REAP=0` to disable entirely.
- **Logging.** Each kill prints one line — timestamp, pid, RSS, cap, full command —
  to the e2e log, so the nightly output shows whether (and how often) it fired.
- **Lifecycle.** Hooked into the **same teardown path** as the rest of the e2e
  session so it never outlives the run: `setup_e2e_session`'s `teardown_e2e` (all
  branches) and the umbrella `scripts/e2e.sh`'s `teardown_e2e` call
  `stop_webkit_reaper`. Under the umbrella, `e2e-browser.sh` also installs its own
  EXIT trap so the loop dies when the browser phase ends, before the wasm/embedder
  phases. `stop_webkit_reaper` is idempotent and pidfile-aware (no-op when nothing
  was started, e.g. in `e2e-api.sh`).
- **macOS-first, degrades gracefully.** `ps`/`kill` behave the same on Linux/CI;
  if `ps` is unavailable the reaper warns and is simply not started, so a Linux
  e2e run is never broken by the guard.
- **Test.** `scripts/lib/webkit_reaper_test.sh` (run directly, no harness — same
  convention as `e2e_test.sh`) exercises the selection + kill logic against
  synthetic over/under-cap rows mapped to real sleeper PIDs: it asserts an
  over-cap WebKit match is killed while an under-cap match, a non-Playwright
  Safari, and a Playwright chromium are all left alone, plus the cap/match knobs
  and the start/stop lifecycle.

## Test Coverage

### Browser E2E (16 tests)
- **Chat** (3): send/receive, thread sidebar, response content
- **Threads** (2): create/switch, message loading
- **Pinning** (3): pin, persist after reload, unpin
- **Reload** (2): message persistence, input usability
- **Streaming** (2): progressive rendering, completion status
- **Empty states** (4): compose view, drawer, health, error handling

### HTTP API (16 tests)
- **Health** (2): status ok, field structure
- **Chat** (3): stream response, event ID, thread targeting
- **Threads** (4): list shape, creation, pin/unpin, messages
- **SSE** (2): connection, event delivery
- **Errors** (5): unknown route, malformed JSON, missing content-type, wrong method, nonexistent thread

## Running the Tests

```bash
# Start the e2e workspace
./scripts/web-dev.sh -w ~/workspaces/e2e-test -b

# Browser E2E tests
./scripts/e2e-browser.sh

# HTTP API tests (also boots the e2e workspace)
./scripts/e2e-api.sh

# Both back-to-back (what the nightly pipeline runs)
./scripts/e2e.sh

# With visible browser (debugging)
./scripts/e2e-browser.sh -h
```
