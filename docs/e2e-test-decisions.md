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

### The e2e database is rebuilt from zero, never truncated
`reset_e2e_database` (`scripts/lib/e2e.sh`) **drops and recreates** the workspace database, so the engine's next boot runs the entire sqlx migration chain against an empty database — seeds included. It used to `TRUNCATE` every table *except* `_sqlx_migrations`, and that one exclusion was the whole problem: seed data lives inside migrations, the surviving `_sqlx_migrations` rows told sqlx they were already applied, so their seed `INSERT`s never re-ran. Any table whose only content comes from a migration seed was therefore permanently empty in e2e. `models` was the casualty — 0 rows where a real workspace has 26 — so `llm::model_registry::load_from_db` built an empty map, `RoutingProvider` silently fell back to `prefix_heuristic` for every model, and nothing derived from the registry (provider routing, per-model `context_window`) was testable in e2e at all. The database was also genuinely long-lived rather than reset: the bootstrap migration in `lucidos_e2e-test` carried an `installed_on` six weeks old.

**The reset therefore owns the engine lifecycle.** Postgres refuses to drop a database with open connections, and migrations, `EventStore::init_schema()` and the pgvector setup all run exactly once — at engine boot (`engine/engine_impl/construction.rs`). So `reset_e2e_database` stops the engine (SIGUSR1, which also ends the supervisor's restart loop), recreates the database, and starts the engine again; on return the workspace is running on a fresh database. Call it **instead of** `ensure_workspace_running`, never before it — booting first would waste a boot against the stale database. It asserts the outcome (`_sqlx_migrations` absent) rather than trusting psql's exit code, which is 0 even for a refused `DROP` unless `ON_ERROR_STOP` is set.

**Cost is the engine restart, not the migrations.** The full 157-file chain applies in ~0.2 s, which is why there is no template database (`CREATE DATABASE … TEMPLATE …`) — it would add a second mechanism with its own staleness rules to save nothing. Each reset costs one engine boot instead, so `build_e2e_engine_once` builds the SDK + engine **once per script invocation** and later restarts only locate that binary: recompiling between Playwright projects would swap the binary out from under a running suite.

This also **replaced** `purge_orphan_migrations`, which dropped the public schema when `_sqlx_migrations` referenced a migration file that no longer existed (abandoned CC branches left orphan rows and sqlx then refused to start with `VersionMissing`). Every resetting run now starts from an empty database, so that case can't arise. It survives only on the paths that deliberately *don't* reset — `--no-reset` and `e2e-ios.sh`, which both attach to whatever database is already there. On those, the developer has explicitly asked to keep it and silently wiping their schema was the wrong answer anyway; the engine's `VersionMissing` names the problem, and running once without `--no-reset` fixes it.

### Single-writer lock on the e2e workspace
Every e2e entry point (`e2e.sh`, `e2e-browser.sh`, `e2e-api.sh`) acquires `~/workspaces/e2e-test/.lucidos/e2e.lock` (PID + `$LUCIDOS_THREAD_ID` + worktree path + start time) before starting the workspace or any browser. A second invocation while the lock is held (owner PID alive) exits 1 with a message naming the holder. The lock exists because two CC sessions running Playwright concurrently against the shared workspace race on browser processes — on 2026-04-19 a WebKit GPU child leaked to 28 GB and OOM-rebooted a 32 GB Mac.

**Reclaiming a stale lock is orphan-safe, not blind.** A "stale" lock is one whose owner PID is dead — but an *interrupted* run (killed before its EXIT trap could tear down) leaves orphaned e2e processes alive: Playwright/WebKit browser children and the e2e-test workspace engine, still holding RSS. The old reclaim treated "owner dead" as "safe to start fresh", so on 2026-06-21 the nightly orchestrator re-spawned the full suite three times and each re-spawn reclaimed the free stale lock and stacked a fresh set of browsers on top of the orphans → 23.5 GB compressed + 14 GB swap, machine pinned in critical memory pressure for 4+ hours. `acquire_e2e_lock` now scans for those orphans before reclaiming (browser children matched by the `ms-playwright/*` cache path — same discriminator the webkit reaper uses; the engine via its own `engine.pid`), runs a **deliberate, logged sweep** (SIGKILL the browsers, SIGUSR1 the engine so its supervisor stops cleanly), re-scans, and reclaims only once they are gone. If the sweep can't clear them it **refuses** with an actionable error rather than stack. The four states: no lock → acquire; live-PID lock → hard-fail; stale + no orphans → reclaim; stale + orphans → sweep then reclaim, else refuse. (Deliberately *not* swept: any `vite`/web-dev server — under ADR 0014 e2e runs a one-shot `vite build`, no long-lived server, and a name-based match would risk SIGKILLing the checkout-level shared build-watch that serves other workspaces.) Lock logic in `scripts/lib/e2e_lock.sh`; covered by `scripts/lib/e2e_lock_test.sh` (run directly, no harness — hermetic, fakes orphans with sleepers, never spawns a browser).

### A run that LOST the lock subscribes; it does not poll
The lock's refusal is correct, and how a loser *waits* was not. On 2026-08-09 three coding-agent threads raced for it at once: one held it mid mobile-webkit run, and both losers hand-rolled a busy-wait. One wrote `/tmp/run-e2e-retry-<pid>.sh` with `for i in $(seq 1 120)` around `./scripts/e2e-browser.sh` and `sleep 20; continue` on refusal, a 40 minute foreground tool call that re-executed the entry script's build checks on every attempt; the other parked on a bare `sleep 20`. Both burned a Claude Code turn and held engine capacity to learn something the engine could have told them.

So **the lock announces itself**. Every hold emits `E2ELockAcquired` when it starts and `E2ELockReleased` when it ends, as domain events through `lucidos events emit`, and a refused run subscribes with `lucidos await-event --on E2ELockReleased` and **ends its turn**. The engine re-opens the thread when the event lands. `.claude/skills/e2e-lock-wait/SKILL.md` carries the agent-facing rules (one-shot subscription, forward-only watch, the cap of 10 consecutive subscriptions, the attempt cap, timeout handling); the refusal message teaches the same path in four lines so an agent that never loaded the skill still does the right thing.

| Event | When | Payload |
|---|---|---|
| `E2ELockAcquired` | a run takes the lock | `script`, `thread_id`, `worktree`, `reclaimed` |
| `E2ELockReleased` | a hold ends | `script`, `thread_id`, `worktree`, `held_secs`, `outcome`: `released` or `reclaimed` |

**Both endings emit, including the reclaim.** A waiter is blocked on the *hold*, and a hold whose owner died is over just as finally: the dead owner's EXIT trap never ran, so the run that reclaims the stale lock emits the release on its behalf with `outcome: "reclaimed"`, describing the dead owner rather than itself. That is what stops a hard-killed holder from stranding every waiter until its own deadline.

**The emit is best effort and bounded, in that order of importance.** A missing `lucidos`, a down engine, a non-zero exit and a hang must not turn a green run red or stall an EXIT trap, so failures are discarded and the wait is capped at `E2E_LOCK_EMIT_TIMEOUT_S` (5s) against a wall-clock deadline. Not a tick count: each `sleep 0.1` costs a fork, ~0.25s on a loaded Mac, so counting iterations bounds the naps rather than the elapsed time and drifts furthest exactly when a teardown is running. The CLI's own 30s reqwest default is both too slow for a teardown and no help against a `lucidos` wedged before its HTTP client exists. The emit is suppressed entirely while `$E2E_LOCK_DIR_OVERRIDE` is set, which is what keeps `e2e_lock_test.sh` from writing `E2ELock*` events into the developer's live workspace. And the release emit is announced ONLY if the `rm` actually succeeded: `rm -f` is silent about a missing file but not about a permission error, and announcing through one wakes every waiter onto a lock that is still held.

**Acquire announces in the BACKGROUND, release in the foreground**, which is not symmetry worth tidying away. `acquire_e2e_lock` returns having taken the lock, and both entry points install their teardown only afterwards, so anything it blocks on widens the window in which an interrupt leaves a stale lock nobody releases and no waiter hears about. `release_e2e_lock` runs inside that teardown with the lock already gone, so blocking there costs teardown time and nothing else, which is exactly what the bound is for.

**Two gaps are accepted rather than closed, and the subscriber's own `--timeout-secs` is the recovery for both.** They are documented here and in the skill because a waiter that does not know about them reads an idle thread as progress:

- **Cross-workspace.** The lock is shared by every workspace on the machine, but `lucidos events emit` writes to the emitting subprocess's own `$LUCIDOS_WORKSPACE`. A holder in workspace A releasing does not wake a waiter in workspace B. Deliberately not closed by a second emit or by hand-rolled HTTP at another engine's port (ADR 0057). `acquire_e2e_lock` compares the holder's `WORKTREE` against `$LUCIDOS_WORKSPACE` and says so in the refusal when it can tell, staying silent when it cannot (no `LUCIDOS_WORKSPACE` to compare against) rather than guessing.
- **Engine down at the moment of release.** The emit is an HTTP POST to a live engine, so nothing is written and there is nothing for the waiter's boot catch-up scan to find. Narrow in practice: a wake only works same-workspace, so the waiter shares that engine and is down with it.

**The subscription itself survives a restart**, and that is the half worth knowing: the persisted `EventWaitStarted` *is* the wait, rebuilt from the event store at boot, carrying the event sequence it was armed at so a release emitted while the engine was down is still delivered on the way back up, and an expired one wakes its thread with a timeout instead of vanishing (`crates/lucidos-engine/src/engine/event_wait/`). Only the emit is mortal.

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
- `scripts/e2e-browser.sh` still phase-splits nav/CC specs, which shrinks
  variant 2's contention window and is harmless to keep. It no longer runs
  `mobile-webkit` FIRST. See "mobile-webkit runs last, and alone" below: the
  memory cost outranks the ordering, and the ordering bought less than it
  looked like it did.

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
`MessageReceived` echo — all gated on `hasUnsentLocalDraft`), it still surfaced once
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

#### Never select a thread row by POSITION — the drawer is not yours alone

The 2026-07-26 nightly then hit `drafts.spec.ts:65` again — first attempt, **five of
five** full runs, always ~35.5s — and the instrumentation above reported
**NOT-STORED**. That verdict was **wrong**, and chasing it would have been a seventh
blind fix in the compose-draft code. A trace of a reproduced failure (2026-07-29)
shows the compose PUT going out on schedule and being accepted:
`PUT /threads/<id>/compose {"text":"thread draft text",…} → 204`. The draft was
stored. The test was looking at a **different thread**.

The mechanism, and the rule it forces:

1. `clearAllThreads()` truncates the `thread_summaries` **projection** — behind the
   engine's back. It does not stop anything the engine still considers live.
2. A coding-agent session started by an EARLIER spec can still be running.
   `coding-agent-question.spec.ts` (mobile-webkit nav chunk 2) answers a CC question;
   the engine dispatches a **Continue** spawn (`--resume` in a fresh worktree) and the
   spec ends immediately on its UI assertion. The real Claude Code subprocess keeps
   working for ~45s.
3. When that session's next event lands — inside the *next chunk's* test — the
   projection's `INSERT … ON CONFLICT DO UPDATE` **re-creates** the row that
   `clearAllThreads()` deleted, with `last_activity = NOW()` and no `first_message`.
   It renders as "Untitled Thread" and sorts **above** the row the running test just
   created.
4. `drafts.spec.ts:65` navigated back with
   `clickVisibleElement(page, '.thread-row:not(.compose-draft-row)')` — *the first
   visible real row*. It clicked the foreign thread, found an empty textarea, and its
   classifier then queried `compose_text` for **that** thread — `''` → "NOT-STORED".

So: **a test that means "the thread I created" must select it by id**, via
`clickThreadRow(page, threadId)` / `threadRowFor(threadId)` (`e2e/helpers.ts`), which
key on the `data-flip-id` the drawer already stamps per row. `REAL_THREAD_ROW` with a
positional `.first()` is only safe where the test genuinely means "any real row" (or
scopes further with `hasText`). A test asserting on a thread it hasn't identified can
report a perfectly healthy product as broken — which is exactly what happened here.

Corollary for diagnostics: **assert identity before value.** The restore step now
asserts the restored prompt's `data-thread-id` matches the thread it typed into
*before* asserting the draft text, so a wrong-thread landing fails as a wrong-thread
landing instead of masquerading as data loss.

Why it looked webkit-only: mobile-webkit is the only project that splits its run into
a nav phase and a CC phase and shards each into fresh-browser chunks. That split
removes the four CC-destination spec files (14 tests) that otherwise sit between
`coding-agent-question.spec.ts` and `drafts.spec.ts`, so on webkit `drafts:65` runs
~40s after the leaked Continue spawn — inside its window. chromium and mobile run one
unsharded alphabetical pass, where those tests (each spawning its own CC subprocess)
push `drafts:65` well past it. Nothing about WebKit itself was involved.

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

### mobile-webkit runs last, and alone

One project costs about **15 GB of macOS VM compressor**. Everything else in the
suite costs about 0.6 GB between them. That asymmetry, not any test, is what
decides the run order.

The reaper below cannot see it. `webkit_reaps` was **0** across a climb from
11.93 GB to 17.27 GB. The reaper caps per-process RSS at 6 GB and no single
WebContent process comes near that, because the cost is spread over many
short-lived ones. `kern.memorystatus_vm_pressure_level` is no use either: it
read normal for the whole climb. Only the compressor moves in time to act on,
so `run_specs_chunked` and the project loop both read it. Both stop over
`LUCIDOS_E2E_COMPRESSOR_MAX_GB` (12 GB).

Three levers were tried, in order of cost.

1. **Smaller chunks.** `LUCIDOS_E2E_WEBKIT_CHUNK` went 8 to 3. It bounds the
   per-chunk delta and it is not enough: a run at 3 still hit the ceiling,
   because it started from 5.90 GB instead of a cold 1.9 GB.
2. **A ceiling between chunks.** Stops the loop at a safe boundary rather than
   riding the climb into a host freeze. This host has no swap, so a harness that
   keeps going into a rising compressor is a machine lock, not a slow test.
3. **Its own run**, which is this section. The other two bound the damage; only
   this one stops the damage landing on somebody else's coverage.

**The compressor does not drain between projects.** It fell 0.55 GB at WebKit
teardown and then stayed within 0.7 GB of its high-water mark for three more
hours. So a WebKit run permanently spends the session's budget, and the two
consequences are separate.

- **Running it last protects everything else.** `scripts/e2e.sh` runs api, wasm
  and embedder before the browser phase, and the browser phase runs `chromium
  mobile mobile-webkit`. Two consecutive nightlies died inside mobile-webkit
  with wasm and embedder never started, which was a coverage hole rather than
  only a resource cost. Whatever is queued behind the expensive project is what
  a stop loses, so nothing is.
- **Running it separately is what helps mobile-webkit itself.** `--no-webkit`
  (on either script) leaves it out, so it can start from a cold host:

```bash
./scripts/e2e.sh --no-webkit          # api, wasm, embedder, chromium, mobile
./scripts/e2e-browser.sh --webkit     # then mobile-webkit, cold
```

An excluded project is dropped from the per-project table rather than recorded
green, and `report_webkit_excluded` says so on every exit path. A run with a
hole in it must not read like a run without one.

**This reversed an earlier deliberate ordering**, which put mobile-webkit first
to keep its contention-sensitive spawns ahead of two more passes of
CC-subprocess churn. That reasoning is weaker than it looks. The wedge it
targeted is fixed at the source, by the explicit `proxy` on the mobile-webkit
project. The projects run sequentially, so the churn was never concurrent with
it. And the phase split inside the project remains the real mitigation.

### Host-load backpressure guard — refuse to launch onto a saturated host

The reaper and the single-writer lock above cover *concurrent* and *runaway
per-process* memory, but neither looks at **system load** before starting. On
2026-07-01 an EXTERNAL macOS daemon (`triald` → `mobileassetd`
purgeable-CacheDelete loop, misfiring at `targetingPurgeAmount:0` with 549 GB free
— a known Tahoe daemon bug, **not** ours) pinned an 18-core box at load ~96. The
nightly e2e step then launched its Playwright browser swarm (WebKit + Chromium)
**on top of** the already-pegged host; the browsers wedged ("failed localhost
commit"), the machine became unresponsive (the user could not even log in), and it
had to be hard-rebooted. We can't fix the daemon — but the e2e tooling piled heavy
work on unconditionally. `scripts/lib/host_load_guard.sh` is the missing guard.

- **What it does.** Before the browser swarm spawns it samples the **1-minute load
  average** portably (`uname -s` → macOS `sysctl -n vm.loadavg` + `sysctl -n
  hw.ncpu`; Linux `/proc/loadavg` field 1 + `nproc`/`getconf _NPROCESSORS_ONLN`)
  and computes `load1 / ncpu` via `awk` (float-exact — `27/18 = 1.5` is NOT over a
  `1.5` cap; `27.1/18` is). Ratio ≤ cap → return 0 immediately (a healthy host
  pays one sample). Ratio > cap → **wait-and-back-off**, polling every
  `HOST_LOAD_POLL_SECS` with a log line each cycle until the ratio drops under cap
  (return 0) or `HOST_LOAD_MAX_WAIT_SECS` is exceeded.
- **Saturated → distinct exit code 75.** If the host is still over-ratio after the
  wait cap, `wait_for_host_load` returns `HOST_LOAD_SATURATED_EXIT` (**75**, the
  `EX_TEMPFAIL` sysexits convention) with a "still saturated … refusing to launch"
  message — distinguishable from an ordinary test failure (`1`) so the nightly
  orchestrator can recognize a backpressure abort.
- **Knobs.** `HOST_LOAD_MAX_RATIO` (default `1.5` — load may reach 1.5× the core
  count before we back off), `HOST_LOAD_POLL_SECS` (default `15`),
  `HOST_LOAD_MAX_WAIT_SECS` (default `300`), and `HOST_LOAD_GUARD_DISABLE=1` to
  make it a no-op (escape hatch for CI where load is meaningless). Tests inject
  readings via `HOST_LOAD_OVERRIDE` / `HOST_NCPU_OVERRIDE`.
- **Fails open.** If it can't MEASURE the host (unknown OS, unreadable load,
  `ncpu` empty/zero/non-numeric) it logs and returns 0 — a guard that can't measure
  must never block or crash the suite (same posture as the reaper's "no `ps` →
  don't start"). `awk` never divides by zero.
- **Wiring.** Invoked **once**, in `scripts/e2e-browser.sh`, right after
  `setup_e2e_session` (e2e lock held) and before `start_webkit_reaper` / any
  Playwright spawn — the single chokepoint that covers BOTH the standalone browser
  run and the umbrella `scripts/e2e.sh` nightly (`e2e-browser.sh` runs under both).
  On a saturated abort the exit-trap chain releases the e2e lock (no stale lock)
  and exit 75 propagates. It is **not** added to `scripts/e2e-api.sh` (lightweight,
  not the pile-up risk) and deliberately not invoked a second time in `e2e.sh`
  (that would double the wait in the nightly and read staler load than gating right
  before the swarm).
- **Test.** `scripts/lib/host_load_guard_test.sh` (run directly, no harness — same
  convention as `webkit_reaper_test.sh`) covers: under-threshold → immediate 0;
  sustained over-cap → 75 after a tiny wait cap (with a bounded-elapsed no-hang
  check); over-cap that recovers mid-wait → 0; `HOST_LOAD_GUARD_DISABLE=1` → 0;
  float-compare exactness at the `1.5`/`1.0` boundary; and empty/zero/non-numeric
  `ncpu` failing open with no divide-by-zero.

### Mid-run host saturation is classified, never mistaken for a product failure

The launch gate above only knows about the instant it fired. On 2026-07-26 it
passed on a quiet host, and then external macOS daemons (an MDM agent plus
`mdmclient` / `mobileassetd` / `managedcorespotlight` — the periodic management
sweep of an MDM-managed corporate fleet) pinned the box at load **83–227 for ~40
minutes MID-RUN**. The browsers starved, and the resulting mobile-webkit timeouts
were indistinguishable — in the log — from real product failures. A human had to
notice the host was busy and re-run.

So `host_load_guard.sh` also samples **throughout** the run:

- **`start_host_load_sampler`** — a background loop (same shape as the WebKit
  reaper's: pidfile, SIGTERM-safe, kills its in-flight `sleep`) appending
  `<epoch> <load1>` every `HOST_LOAD_POLL_SECS`. Started by `e2e-browser.sh` next
  to the reaper; truncates any previous run's samples at start.
- **`report_host_load_saturation RUN_RC`** — drains the samples at the end.
  **Always** prints a one-line summary (peak load, peak ratio, samples over cap,
  longest sustained stretch) so triage has the evidence either way. Prints a loud
  banner **only** when the run FAILED *and* the host was over the cap for at least
  `HOST_LOAD_SUSTAINED_MIN_SECS` (default 120) contiguously.
- **One threshold, not two.** Saturation is judged against the same
  `HOST_LOAD_MAX_RATIO` the launch gate uses, through the same reader/compare
  helpers. `HOST_LOAD_SUSTAINED_MIN_SECS` is a *duration*, not a second
  threshold — it exists so an isolated spike (a heavy chunk, the release build's
  tail) can't be blamed for a failing run.
- **No retry, no swallowed exit code.** The reporter always returns 0 and the
  caller exits with its own code; a saturated run still fails. The banner says
  those failures are not trustworthy evidence and to re-run on an idle host — it
  does not make them disappear. Auto-retrying the suite was considered and
  rejected: it converts an honest "we can't tell" into an expensive guess.
- **Wiring.** `e2e-browser.sh` funnels all three of its exit paths (full webkit,
  caller-pinned project, the per-project loop) through one `finish` helper that
  stops the sampler, reports, then exits with the run's own code.
  `stop_e2e_background_guards` (`scripts/lib/e2e.sh`) stops the reaper and the
  sampler together and is called from every teardown, so an abnormal exit can't
  orphan either loop.
- **Test.** `scripts/lib/host_load_guard_test.sh` covers: the sampler records and
  stops cleanly (and leaves the samples for the report); a start truncates a
  crashed predecessor's samples; failed + sustained → banner quantifying peak and
  duration; failed + a 30s spike → **no** banner; a green run → never a banner;
  raising `HOST_LOAD_MAX_RATIO` suppresses the banner (proving the shared cap);
  and no samples / garbage samples / unreadable core count all fail open.

### Failure traces survive the whole run — one `--output` dir per invocation

`trace: 'retain-on-failure'` + `screenshot: 'only-on-failure'` are the only evidence
an *unattended* nightly failure leaves behind, and they were being destroyed before
anyone could read them. (Playwright calls these "output artifacts"; they are NOT
Lucidos *artifacts* — they're ephemeral gitignored test output, so this section says
traces/screenshots and the script names its variables `output`.) Playwright deletes its output dir at the **start** of every
`playwright test` invocation (`createRemoveOutputDirsTask`), and the default is the
whole `test-results/` tree — but one suite run makes many invocations: one per
project (`mobile-webkit`, `chromium`, `mobile`) plus one per mobile-webkit chunk. The
chunks already had per-chunk `--output` dirs; the per-project passes did not, so the
`chromium` pass wiped every webkit chunk's traces and the `mobile` pass then wiped
chromium's. Only the LAST project's survived. A targeted repro run afterwards
(`-f <spec>`) defaulted to `test-results` too, so triaging destroyed what was left.

`scripts/e2e-browser.sh` now wipes **one root once**, up front, and gives every
invocation its own subdir under it (`set_output_dir`) — nothing is wiped mid-run:

- Full run → `test-results/full/<project>` and `test-results/full/<project>-<phase>-<n>`,
  with the whole `test-results/` tree cleared once before the first project.
- Targeted run (`-f`, or any `--` passthrough) → `test-results/targeted/…`, and it
  clears **only** that root, so a preceding full run's evidence — usually the very
  thing you're reproducing against — stays intact.
- A caller-supplied `--output` is detected and never overridden.

There is no `--preserve-output-dir` CLI flag to reach for (the runner option exists
but is internal), so per-invocation `--output` is the only lever.

### The harness may not report a status it does not have

A test harness has one job beyond running tests: telling the truth about what it
ran. The 2026-07-26 nightly exposed a case where it could not. `e2e-browser.sh`
prints a per-project exit-code table at the end of a full run, and the LAST
project's cell was always blank — including on a run where `mobile` had two real
failures. The umbrella exit code was computed independently and stayed correct, so
nothing was masked, but a signal that is silently blank on every run is not a
signal.

**Root cause: a leaked loop variable.** The project loop is
`for i in "${!PROJECTS[@]}"` and recorded results with `PROJECT_RCS[$i]=$rc`. Its
body calls `reset_e2e_database` → `ensure_workspace_running`, whose frontend
readiness poll was `for i in {1..30}` with **`i` not declared `local`**. Bash
function locals are opt-in, so that counter overwrote the caller's index: every
iteration after the first wrote its result to the poll count's slot (typically 1)
instead of its own, so `chromium`'s entry was overwritten by `mobile`'s and index 2
was never created. `for` re-assigns `i` from the word list at the top of each
iteration, which is why the loop itself still visited all three projects.

Three changes, because one of them alone would leave the class open:

1. **`local i`** in `ensure_workspace_running` — the actual fix — plus every other
   loop variable that leaked from a lib on the same call path (`cleanup_e2e_worktrees`,
   `start_engine`, `_start_postgres_container`, `resolve_workspace`,
   `running_frontend_workspaces_in_project`, `cleanup_stale_sleep_locks`,
   `check_prereqs`). The invariant — *every loop variable in a sourced lib function
   is `local`* — is **enforced, not asserted**: `test_no_sourced_lib_leaks_a_loop_variable`
   (`scripts/lib/e2e_test.sh`) scans all eight libs `scripts/lib/e2e.sh` pulls in and
   fails on a new leak, and plants a leaky fixture to prove the scan isn't vacuous.
   `_` is exempt — bash reassigns it after every simple command, so no caller can
   rely on it.
2. **`PROJECT_RCS+=("$rc")`** instead of an indexed write — appending in lockstep
   with `PROJECTS` cannot produce a hole, so a future leak in any lib function on
   that call path can't corrupt the table.
3. **`report_project_exit_codes`** (`scripts/lib/e2e.sh`) — an entry whose rc is
   empty or non-numeric prints `UNKNOWN (harness bug)` and **forces the umbrella
   exit code non-zero**. A run whose per-project status is unknown must not report
   green. It lives in the lib rather than inline in `e2e-browser.sh` precisely so
   it is unit-testable (`scripts/lib/e2e_test.sh`) — a guard against harness bugs
   with no test of its own is the same bet that produced the bug.

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

Each script builds the engine + SDK and boots its own session-scoped engine for the
disposable `e2e-test` workspace, then tears it down — there is no separate start
step, and `web-dev.sh` must not be used to pre-start one (it launches the
machine-global gateway; refused from a coding-agent worktree — ADR 0021).

```bash
# Browser E2E tests
./scripts/e2e-browser.sh

# HTTP API tests
./scripts/e2e-api.sh

# Everything back-to-back (what the nightly pipeline runs)
./scripts/e2e.sh

# With visible browser (debugging)
./scripts/e2e-browser.sh -h
```

Failure traces + screenshots land under `crates/lucidos-app/test-results/full/…`
(or `…/targeted/…` for a filtered run) — see the per-invocation `--output`
subsection above.
