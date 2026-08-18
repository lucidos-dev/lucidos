# Known Gaps

Living registry of **known functional gaps and platform limitations** — things
that don't work (or work only partially) today that a user or developer might
reasonably expect to. Distinct from its neighbours:

- **`docs/temporary-measures.md`** — impermanent code that carries a *concrete
  removal condition*. A gap here may graduate into a temporary measure once
  someone starts closing it.
- **`docs/code-review-priors.md`** — review findings dismissed with evidence.
- **`docs/adr/`** — decisions, including deliberate *no*s.

A gap belongs here when it's a real limitation with no scheduled fix (permanent
platform limits, or work explicitly deferred). Add an entry in the **same change**
that discovers or creates the gap. When a gap is closed, flip its **Status** to
`closed (<date>)`, name the change, and keep the row as history.

Entry shape: **Where · Gap · Why · Status / workaround.**

---

## Pane focus & content-pane iframes

### Tauri native browser (URL preview) does not move the content focus marker
- **Where:** `crates/lucidos-app/src/components/files/UrlPreviewInline.tsx` (Tauri
  branch → native overlay webview via `create_panel_webview` in
  `crates/lucidos-app/src/lib.rs`); the host focus tracker
  `installContentPaneIframeFocusTracking` in
  `crates/lucidos-app/src/components/layout/paneFocus.ts`.
- **Gap:** Clicking the internal browser in the packaged Tauri desktop app does
  not move the focused-pane marker to `content` — the marker stays on the
  previously focused pane.
- **Why:** The Tauri URL preview is a NATIVE child webview overlaid on the pane,
  not a DOM `<iframe>`. The host-side tracker keys on `window` `blur` +
  `document.activeElement` being a content-pane `<iframe>`; a native webview is
  invisible to both. App iframes and file/HTML/PDF previews ARE DOM iframes, so
  they are covered.
- **Status:** Open — deferred by design decision (2026-07-02, user-approved).
  Closing it needs native focus wiring in `lib.rs` (a child-webview
  focus/pointerdown hook emitting an event the frontend maps to
  `focusedPane = 'content'`), verifiable only in a packaged build (WKWebView can't
  be UI-automated — ADR 0016). Marginal benefit: while the native webview holds OS
  focus the host webview isn't receiving keyboard input, so the host keyboard nav
  the marker drives doesn't apply anyway.

### Host keyboard shortcuts don't work inside a cross-origin URL preview
- **Where:** `crates/lucidos-app/src/components/files/UrlPreviewInline.tsx`
  (browser-mode `<iframe>` branch); `previewIframeShortcuts.ts`.
- **Gap:** While focus is inside a browser-mode URL preview (an external site),
  the shell's keyboard shortcuts (pane focus/hide, narrow/widen, Escape, …) do
  nothing and the chord falls through to the browser's own default.
- **Why:** The preview loads a cross-origin external URL. The keydown bridge
  (`bridgePreviewIframeShortcuts`) needs same-origin `contentDocument` access,
  which cross-origin blocks; and unlike an app, an external page runs no Lucidos
  SDK to postMessage-forward its chords. Same-origin previews (file/HTML/diff/PDF)
  and app iframes are both covered. The content focus MARKER *does* move (via the
  window-blur tracker) — only the shortcut forwarding is missing.
- **Status:** Open — fundamental cross-origin limitation; no clean fix.

### Many sites refuse to load in the browser-mode URL preview
- **Where:** `crates/lucidos-app/src/components/files/UrlPreviewInline.tsx`
  (non-Tauri iframe branch).
- **Gap:** Opening some URLs in the in-app browser (non-desktop / browser mode)
  shows a blank frame.
- **Why:** Sites that send `X-Frame-Options: DENY`/`SAMEORIGIN` or a CSP
  `frame-ancestors` directive can't be embedded in an iframe. The Tauri desktop
  build sidesteps this with a native webview; the browser build can't.
- **Status:** Open — platform limitation of iframe embedding (the code comment
  already notes "many sites block framing, but it's the best we can do without a
  native webview").

## Notifications & push

### iOS web push requires an installed (home-screen) PWA
- **Where:** `crates/lucidos-app/src/store/actions/push.ts` (`isIOS() && !isStandalone()` guard), `system-knowhow/notifications.md` §1
- **Gap:** A user browsing Lucidos in an iOS Safari *tab* cannot enable push notifications — the enable flow refuses with "On iOS, add Lucidos to your home screen first to enable push notifications".
- **Why:** iOS Safari only exposes the Service Worker / Web Push APIs to a PWA installed to the home screen (standalone display mode); a plain browser tab has no service worker to subscribe with.
- **Status:** Open — user must "Add to Home Screen" first. Structural iOS/WebKit limitation, no engine-side fix.

### Web-push banners can't be silently dismissed cross-device on the open web
- **Where:** `system-knowhow/notifications.md` (§ on cross-device dismiss), `crates/lucidos-engine/src/engine/event_bus.rs` `NativePushDismissRequested`
- **Gap:** Reading a notification on one device does not clear the already-delivered banner on the user's other browser/PWA devices. Cross-device dismiss works only for the native macOS desktop app.
- **Why:** Safari revokes a Web Push subscription after 3 *silent* pushes, and Chrome/Firefox show a default "site updated in background" banner on a silent push — so a silent "remove that banner" push is not a usable mechanism on the open web. Only the Tauri desktop app can silently remove a delivered native banner (`removeDeliveredNotifications`).
- **Status:** Open — browser/PWA banners persist until manually swiped. Platform limitation of Web Push.

### Native macOS notifications require a signed `.app` — inert in `tauri dev`
- **Where:** `crates/lucidos-app/src/notifications.rs` (module header + `tauri::is_dev()` early-return)
- **Gap:** Running the desktop app via `tauri dev` (unbundled `cargo run`) delivers **no** native macOS banners; only browser/PWA clients get web push during dev.
- **Why:** Apple's supported `UNUserNotificationCenter` API throws for an unbundled binary — `currentNotificationCenter()` requires the process to run inside a signed `.app` bundle. (The deprecated `NSUserNotification` path Apple has dismantled and no longer delivers on recent macOS.)
- **Status:** Open — test native notifications only in a packaged `.app`. Platform requirement.

### The desktop app gets no native notifications off macOS
- **Where:** `crates/lucidos-app/src/notifications.rs` (`#[cfg(not(target_os = "macos"))]` no-ops for `setup`/`show`/`dismiss`/`set_dock_badge`/`set_tray_title`), `system-knowhow/notifications.md` §1/§4
- **Gap:** On a Linux/Windows desktop build the app surfaces no native banners, dock badge, or tray-title unread count.
- **Why:** The embedded WKWebView can't subscribe to Web Push (no service worker), so the engine reaches the desktop app over SSE and renders banners through Apple's macOS-only `UserNotifications` framework — there is no cross-platform native notification path wired. Browser/PWA clients on those platforms still get web push server-side.
- **Status:** Open — only the macOS desktop app has a native path; other desktops rely on a browser/PWA client for notifications.

### iOS PWA push tap forces a full reload, and can fail to deep-link
- **Where:** `system-knowhow/notifications.md` (tenth iteration + the running+icon-launched section), `crates/lucidos-app/public/sw.js`
- **Gap:** Tapping a push notification on an installed iOS PWA always performs a full cross-document page reload; and when the PWA is already running *and* was last launched from its home-screen icon, the tap can fail to navigate to the notification's target (it just focuses).
- **Why:** WebKit implements neither `launchQueue`/`launch_handler: focus-existing` nor a same-document declarative navigate to an already-open window, so a cross-document query-string URL (a full reload) is the only channel. The running+icon-launched failure is a documented upstream iOS WebKit bug where `notificationclick` never fires — no reliable client-side cure.
- **Status:** Open — upstream WebKit limitation; the reload *is* the navigation, and the deep-link edge case has no client-side fix.

## Packaged vs dev

### Gateway self-reload control is dev-only
- **Where:** `.claude/rules/dev-runtime.md` § "Gateway self-reload"; gateway control status `packaged` field (`crates/lucidos-gateway/src/`), `crates/lucidos-app/src/desktop.rs` (`LUCIDOS_PACKAGED=1`), `crates/lucidos-app/src/components/.../WorkspacePicker.tsx`
- **Gap:** The workspace picker's "reload the gateway onto the rebuilt binary" control (`POST /~/api/v1/control/gateway/reload`, re-exec in place) is hidden in packaged builds.
- **Why:** In-place re-exec onto a rebuilt on-disk binary only makes sense in dev (a CC Apply rebuilds the gateway under a running gateway). A packaged build never rebuilds in place — its updates go through the app updater + a full launchd service restart.
- **Status:** Open by design — packaged gateway updates are delivered by the updater, not an in-place reload.

### Frontend-only in-process Apply refresh is dev-only
- **Where:** `crates/lucidos-engine/src/engine/frontend_refresh.rs` (`refresh_served_frontend_after_rebuild` and the peer `spawn_served_frontend_sync` both early-return packaged/headless), `.claude/rules/dev-runtime.md` § "Exception — a frontend-only Apply…" and § "Shared build-watch"
- **Gap:** The fast path where a pure frontend change advances the served client without an engine restart exists only in dev — both for the applying workspace (in-process re-snapshot on Apply) and for **peer** workspaces (a ~10s periodic re-snapshot of the checkout-shared `dist/` when INV-A-safe, emitting `ServedFrontendAdvanced`); packaged Applies don't get an in-process client swap.
- **Why:** In dev the engine serves a pinned snapshot of a live `dist/` and can re-snapshot in place. Packaged serves an immutable bundled Resources directory that is already one unit — there is no live `dist/` to re-snapshot, so the refresh is a no-op there. (Packaged is also single-workspace-per-install, so the cross-workspace peer case doesn't arise.)
- **Status:** Open by design — packaged frontend updates ship via the updater's full restart. The cross-workspace peer propagation gap (a peer serving a stale client with no badge) is **closed in dev** by the periodic sync — see `docs/plans/2026-07-03-cross-workspace-frontend-only-refresh.md`.

### No HMR anywhere; no Vite in the serving path
- **Where:** `crates/lucidos-app/src/main.tsx` (`import.meta.hot` path inert under built serving), `.claude/rules/dev-runtime.md` § "Frontend: the engine serves the built `dist/`", ADR 0014
- **Gap:** There is no hot-module-replacement in the running app; a code change is picked up by a full browser refresh after the build-watch republishes `dist/`.
- **Why:** ADR 0014 removed the live Vite dev server from the serving path — the engine serves the built `dist/` directly. The build-watch runs a fresh `vite build` per change instead of incremental HMR; the old `import.meta.hot` path is inert under built serving.
- **Status:** Open by design — reload to pick up a change (rebuild is typically sub-second).

### The default `curl | sh` installer 404s today
- **Where:** `install.sh` (download-and-run default), `.github/workflows/release-tarballs.yml` (the attach step), `.claude/rules/build-release.md` § Installer "Caveat"
- **Gap:** The default installer path downloads `lucidos-<version>-<triple>.tar.gz` from GitHub Releases, but no release asset was published, so the download returned 404.
- **Why:** The release workflow uploaded tarballs as **workflow artifacts only** and never auto-attached them; attaching to a Release was behind a manual `workflow_dispatch` + tag ref + opt-in flag.
- **Status:** **Closed (2026-07-30)**, and its residual closed too (2026-08-04). Attaching became automatic on 2026-07-30 and every release from v0.16.0 on carries all four per-platform tarballs + `.sha256` sidecars, but the attach landed 11 to 35 minutes AFTER the Release was cut, so a download started inside that window still 404s (v0.21.0: published 15:49:28Z, Intel Mac tarball attached 16:24:14Z). That window is gone: the tag push is now what attaches, the Release is created as a DRAFT and published only once all four are on it, so a release is complete at the moment it becomes public. `download_failed` still offers `--version <older>` / `--dev` / `--from-tarball` for the other causes.

### Linux / cross-arch releases come only from the CI matrix
- **Where:** `scripts/build-headless.sh` (native-only; `--triple` must equal the host), `.github/workflows/release-tarballs.yml` (per-arch runners, `ubuntu:22.04` container, "Assert portability floor")
- **Gap:** You cannot locally produce a Linux (or cross-architecture) release tarball for a triple other than the build host's, and the resulting binaries won't start on distros older than the glibc 2.35 floor (pre Ubuntu 22.04 / Debian 12 / RHEL 9).
- **Why:** `build-headless.sh` compiles natively (relocatable Postgres + pgvector are fetched/compiled per platform), so cross-arch artifacts must come from CI's per-arch native runners; the Linux entries build inside `ubuntu:22.04` to pin the glibc floor, and a symbol above it fails the build.
- **Status:** Open — it constrains the *build*, not the install: CI attaches all four triples to every published Release, so a non-host Linux/arch user installs normally. What you cannot do is produce that tarball locally for another triple; on an older-glibc distro use `--dev` (build from source on the target), since the prebuilt tarballs don't support it.

### A directly-launched engine can inherit another workspace's gateway identity
- **Where:** `scripts/lib/workspace.sh` (`start_engine`, the direct-front launch used by `web-dev.sh` / `tauri-dev.sh` / the e2e harness), against the three vars only `crates/lucidos-gateway/src/stack.rs` is supposed to set: `LUCIDOS_WORKSPACE_ID`, `LUCIDOS_GATEWAY_PORT`, `LUCIDOS_API_PORT`.
- **Gap:** `start_engine` spawns the engine with the ambient environment, so an engine launched from a shell that already carries those vars adopts a foreign workspace's identity. A coding-agent session is exactly such a shell: the engine that spawned it exports its own, so `./scripts/e2e-api.sh` run from inside one launches the `e2e-test` engine believing it is `dev`. Observed 2026-08-10, when the `e2e-test` engine reported the `dev` workspace's display label.
- **Why:** `crates/lucidos-engine/src/api/base_path.rs` documents these as "set by the gateway when it spawns this engine", and the gateway does write all three from one registry row. Nothing enforces that on the direct-launch path, which predates the gateway and passes the environment through untouched.
- **Status:** Open, with the label surface narrowed. `GET /api/v1/workspace-label` no longer trusts the slug alone: it accepts a registry row only when the row's `port` is also the port this engine serves, which a foreign identity fails (`api/workspace_label.rs`, `a_slug_we_inherited_rather_than_earned_gets_no_label`). That works because the port is rewritten per workspace by every direct launcher (`swap_ports`) while an inherited slug is not, so a leak splits the pair; it is a second piece of evidence rather than a stronger one, and a launch that rewrites NEITHER (a bare `cargo run` from an agent session, onto a port its real owner has released) would still match both halves. The other two consumers are unguarded and one is destructive: `api::history::restart_via_gateway` would ask the gateway to restart the workspace named by the inherited slug, and `boot_report` / `boot_failure` would file phases under it. Closing it properly belongs in the harness, and the choice is not obvious: unsetting the vars in `start_engine` is right for a truly standalone engine but would strip the metas from a `web-dev.sh` workspace the gateway later adopts, so the launcher likely has to set them to the truth for the workspace it is starting rather than clear them.

## Testing & automation

### The packaged macOS app's WKWebView UI can't be automated
- **Where:** `docs/adr/0016-packaged-tauri-e2e-boot-smoke-test.md`, `scripts/e2e-packaged.sh`, `docs/e2e-test-decisions.md`
- **Gap:** There is no end-to-end UI test that drives the *actual packaged macOS window* — panel webviews, native menus, tray, notification taps. `e2e-packaged.sh` asserts only the headless boot chain (service role, embedded Postgres, gateway/engine health, static serving).
- **Why:** Apple's WKWebView exposes no WebDriver, and `tauri-driver` supports only Linux/Windows — there is no supported way to drive the packaged window in CI on macOS.
- **Status:** Open — the packaged window's UI is covered only by the boot smoke test + unit tests of the non-UI Tauri logic; the in-window UI is manually verified.

### The packaged smoke test is opt-in, heavy, and macOS-only
- **Where:** `scripts/e2e-packaged.sh` (Darwin-only `SKIP` guard), `scripts/e2e.sh` (`--packaged` / `LUCIDOS_E2E_PACKAGED=1`), `docs/e2e-test-decisions.md`
- **Gap:** The packaged build smoke test does not run in the default `./scripts/e2e.sh` suite, and skips entirely (exit 0) on non-macOS — so a Linux CI run never exercises a packaged boot chain.
- **Why:** Building the `.app` is a full release engine+gateway build + a relocatable Postgres download + a frontend build + `cargo tauri build` — too heavy for every run; and the `.app` + embedded Postgres bundle is macOS-first.
- **Status:** Open by design — the nightly opts in via `--packaged`; there is no packaged-runtime smoke test for Linux.

### Real-embedder tests are feature-gated and skip on network outage
- **Where:** `.claude/rules/testing.md` § Real-Embedder Tests, `scripts/e2e-embedder.sh`, `crates/lucidos-engine/src/memory/fastembed.rs` (`is_model_fetch_failure`)
- **Gap:** Tests that exercise the real embedding model's semantic behaviour (cross-lingual similarity, Norwegian synonyms, ranking) don't run under a plain `cargo test`, and can silently skip.
- **Why:** They sit behind the `real-embedder-tests` Cargo feature and download ~465 MB from huggingface.co on a cold cache; to stay resilient to HF outages, a cold cache + unreachable HF degrades the test to *skipped* (returns `None`) rather than failing.
- **Status:** Open by design — run `./scripts/e2e-embedder.sh` to exercise them; a cold-cache offline run skips rather than covers.

### The mobile-webkit navigation wedge is mitigated, not eliminated
- **Where:** `docs/e2e-test-decisions.md` § "mobile-webkit navigation wedge" (Variant 2), `scripts/lib/webkit_reaper.sh`, `crates/lucidos-app/playwright.config.ts`
- **Gap:** A residual WebContent cold-start stall can still hang a `mobile-webkit` (iOS Safari emulation) e2e page under heavy host contention.
- **Why:** Variant 2 is intermittent and load-dependent and only reliably clears on a fresh browser context (the whole-test `retries: 1`); the RSS reaper is a safety net against host memory exhaustion, not a cure for the wedge itself.
- **Status:** Open — mitigated via preflight health check, retry, and reaper; not deterministically fixed (browser-side).

### `bash_background` unit tests are load-sensitive and flake under host contention
- **Where:** `crates/lucidos-engine/src/engine/tools/bash_background.rs` `#[cfg(test)] mod tests` — notably `killed_task_preserves_output_written_before_kill` (:547), `drain_returns_only_new_output_each_call` (:446), `wait_zero_matches_legacy_non_blocking_drain` (:870).
- **Gap:** These spawn real subprocesses and poll for their output within fixed windows (e.g. "task did not finish within 8s after kill"). On a saturated host they intermittently fail with `first stdout: ""` / `stdout: ""` / the 8s timeout, and *which* test fails varies per run. Observed 2026-07-26 while the machine was running continuous cargo + vite builds: 1 failure in a full-suite run, then across three consecutive module-only runs — ok, 2 failures (a different pair), ok. The same test passes reliably when run alone.
- **Why:** The assertions are wall-clock races against real process scheduling, not logic. Under contention the child simply hasn't been scheduled to write before the deadline. Making them deterministic means restructuring around a readiness signal rather than a sleep/poll window — a change to the tested module's test harness, not to any caller.
- **Status:** Open — unrelated to any feature work that trips it, so don't chase it from an unrelated diff. Re-run the module (or the single test) to confirm a failure is this flake before treating it as a regression; a genuine regression fails deterministically and names the same test every time.

### The API e2e apply tests race the suite's other tree writers
- **Where:** `crates/lucidos-e2e/tests/api_support/changes_test.rs` (`sequential_apply_two_changes_succeeds`), `app_coding_agent_test.rs` (`app_coding_agent_concurrent_apply`), against the `workspace_tree_lock()` contract in `api_support/mod.rs`.
- **Gap:** Both apply tests merge a branch into the e2e workspace's repo, and a merge refuses a dirty tree. Several other tests in the same parallel run legitimately create and delete non-ignored files there (the trigger tests' script files, the CLI data-write test, the app-seeding helper), so an apply that lands inside one of those windows fails with `Cannot merge: the repository has uncommitted changes`. Observed 2026-08-10: both failed together on one run and both passed on an immediate re-run of the identical tree.
- **Why:** `workspace_tree_lock()` exists for exactly this hazard but is currently written for one reader, the whole-tree snapshot test, which takes `write()` while the writers take `read()`. The apply tests need the same exclusion for the same reason (they too depend on the tree being still) and do not take it, so nothing serialises them against the writers.
- **Status:** Open. The fix is to have the two apply tests take a guard the writers already respect, which means promoting them to `write()` holders of the same lock. Until then, treat a `Cannot merge: ... uncommitted changes` failure in either test as this race and re-run before chasing it; a real regression reproduces.

## Mobile vs desktop

### Keyboard input features are desktop-only no-ops on mobile
- **Where:** `crates/lucidos-app/src/store/actions/pane.ts` (`toggleMaximizeFocusedPaneGroup`, `focusOrToggleThreadDrawer`, `stepThreadPaneWidth`/`stepThreadDrawerWidth`/`resetPaneLayout` — all `if (isMobile()) return;`), `crates/lucidos-app/src/hooks/useKeyboardShortcuts.ts` (`shouldTypeToFocusPrompt` false on mobile)
- **Gap:** Pane maximize (⌘⇧↵), drawer focus (⌘⇧1), pane narrow/widen/reset, and type-anywhere-to-focus-the-prompt do nothing on mobile.
- **Why:** Mobile navigates panes by swipe and has no `focusedPane` signal, no split divider to resize, and its on-screen keyboard / IME composition is incompatible with type-to-focus.
- **Status:** Open by design — these are desktop-only interactions with no mobile equivalent.

### `data-tooltip` tooltips are desktop-only
- **Where:** `.claude/rules/frontend.md` ("No native tooltips: Use `data-tooltip`. Desktop-only.")
- **Gap:** Buttons/icons that carry `data-tooltip` reveal no hint on touch devices.
- **Why:** The tooltip system is hover-driven; touch devices have no hover state, and there is no universal longpress-tooltip fallback.
- **Status:** Open — mobile users don't get tooltip text; important affordances need a visible label instead.

### The thread drawer overlay is desktop-only
- **Where:** `crates/lucidos-app/src/components/drawer/ThreadDrawer.tsx` ("The drawer overlay (threadDrawerOpen) is desktop-only")
- **Gap:** The overlay thread drawer (browse/keyboard-nav threads while viewing one) does not exist on mobile.
- **Why:** On mobile, threads are a separate swipe-navigated pane rather than an overlay on the thread pane — a fundamentally different layout.
- **Status:** Open by design — mobile thread navigation is pane-based.

## Offline / PWA / service worker

### The live event stream isn't served by the service worker
- **Where:** `crates/lucidos-app/public/sw.js` (SSE bypass — "intercepting it hangs the worker — let the browser handle it natively")
- **Gap:** Real-time updates (new messages, status changes) require an online connection; nothing is delivered offline.
- **Why:** The SSE stream (`/api/v1/events`) is deliberately not intercepted — keeping the SW alive for the whole streaming connection hangs the worker when two SW versions coexist during an update.
- **Status:** Open by design — SSE is online-only; the UI reconnects when back online.

### App-UI iframes aren't cached — apps can't open offline
- **Where:** `crates/lucidos-app/public/sw.js` ("app-UI iframes (`/app/<id>/`) … are NOT the SPA shell — they must reach their own server-rendered HTML")
- **Gap:** Opening an app while offline fails; the iframe can't load its engine-rendered HTML.
- **Why:** The SW's network-first shell cache is scoped to exactly `/` to avoid poisoning the shell URL with stale app HTML, so app-UI navigations must reach their own server fresh and are not cached.
- **Status:** Open — apps require network to open.

### Non-GET requests aren't SW-intercepted; iOS body cloning is unreliable
- **Where:** `crates/lucidos-app/public/sw.js` (GET-only interception; iOS WebKit body-stream cloning note — large bodies reject with "TypeError: Load failed")
- **Gap:** Mutations (POST/PUT/PATCH/DELETE — send message, upload image, etc.) get no service-worker retry/offline handling, and large-body mutations can fail outright on iOS.
- **Why:** iOS WebKit can't reliably clone a request body when `respondWith` re-issues the request, so the SW leaves mutations to the network directly.
- **Status:** Open — mutations need a live connection; large uploads on iOS depend on WebKit's own handling.

### Cold offline boot depends on a prior online session
- **Where:** `crates/lucidos-app/public/sw.js` (`networkFirstShell` — "the cache is the offline fallback only")
- **Gap:** A first-ever cold start with no network cannot boot the app.
- **Why:** The navigation shell (`index.html`) is served network-first so it always matches the server's current `/assets/*` bundles; the cache is only an offline fallback populated by a prior successful online load.
- **Status:** Open by design — offline start requires a previously-cached session (chosen over a stale-shell black-screen failure mode).

## App iframes & JS SDK

### App-iframe keyboard chords can't suppress the browser default
- **Where:** `.claude/rules/frontend.md` (SDK keyboard forwarding — "that path can't suppress the browser default (it has no synchronous event to cancel)"), `packages/lucidos-sdk/src/keyboardForward.ts`
- **Gap:** A host shortcut chord pressed while focus is inside an *app* iframe still triggers the browser's own default for that combo (e.g. a context menu), even though the Lucidos action also runs.
- **Why:** An app iframe forwards chords up over `postMessage`, which is asynchronous — by the time the host receives it there is no live event to `preventDefault()`, so the browser default can't be cancelled. (Same-origin *preview* iframes are bridged directly and can cancel it; apps can't.)
- **Status:** Open — cross-origin/async boundary limitation of the SDK forwarding path.

### Screenshot capture fails on modern CSS colours → DOM-only
- **Where:** `packages/lucidos-sdk/src/capture.ts` (html2canvas throws on `color()`/`oklab()`/`oklch()`/`color-mix()`; degrades to a DOM snapshot)
- **Gap:** An app that uses CSS Color 4 functions (common in modern design tokens) gets no rasterized screenshot from `lucidos._capture()` — only a text DOM snapshot reaches the agent.
- **Why:** html2canvas predates CSS Color 4 and throws on those functions; capture is best-effort and falls back to a geometry+classes DOM walk that can't fail on colours it can't parse.
- **Status:** Open — visual screenshot unavailable for such apps; DOM snapshot is the fallback.

### App toasts can't carry action-button callbacks
- **Where:** `packages/lucidos-sdk/src/ui.ts` (`toast` — "The host's action-button callbacks can't cross the postMessage boundary")
- **Gap:** `lucidos.ui.toast()` from an app can only show a message + severity + basic options — no clickable action buttons like the host's own toasts.
- **Why:** Callback functions aren't serializable across the iframe `postMessage` boundary; only plain data crosses.
- **Status:** Open — apps get message-only toasts.

## LLM providers & models

### Vertex ADC: only `authorized_user` credentials work in-engine
- **Where:** `crates/lucidos-engine/src/llm/vertex/adc.rs` (module header), CLAUDE.md § Environment Variables (other types "fall back to the `gcloud` subprocess (dev only)")
- **Gap:** Vertex authentication with `service_account`, `external_account`, or `impersonated_service_account` ADC credential types doesn't work in a packaged/headless engine.
- **Why:** Only the `authorized_user` ADC type is parsed and refreshed in-engine; other types return `None` and require falling back to the `gcloud` CLI subprocess, which a packaged build doesn't ship.
- **Status:** Open — packaged/headless Vertex needs an `authorized_user` ADC (`gcloud auth application-default login`); other types are dev-only via the `gcloud` binary.

### Responses-API models drop image content
- **Where:** `crates/lucidos-engine/src/llm/openai/responses.rs` (`ContentBlock::Image { .. } =>` "Responses API (codex models) doesn't support images — skip")
- **Gap:** Sending an image to a model that uses OpenAI's Responses API (GPT-5+ / Codex) silently omits the image.
- **Why:** The Responses API wire format the engine emits has no image support, so image blocks are skipped when building the request.
- **Status:** Open — images reach only Chat-Completions / Anthropic-format models.

### OpenRouter and local providers are Chat-Completions-only, with no routing heuristic
- **Where:** `crates/lucidos-engine/src/llm/openai/mod.rs` (`force_chat_completions` for OpenRouter/local), `crates/lucidos-engine/src/llm/model_registry.rs` ("no rule for these shapes, so it falls back to Vertex (documented limitation)")
- **Gap:** A GPT-5+/Codex-class model served via OpenRouter or a local OpenAI-compatible server is pinned to the Chat Completions API (never the Responses API); and if such a model id is missing from the `models` table, provider routing falls back to Vertex rather than the intended backend.
- **Why:** OpenRouter/local backends implement only Chat Completions, so `force_chat_completions` is set; and the prefix routing heuristic recognizes only `gpt-`/`claude-` shapes — OpenRouter (`z-ai/*`) and local ids have no prefix rule, so an exact registry hit is required.
- **Status:** Open — keep OpenRouter/local models present in the registry; Responses-API-only features are unavailable through them.

### Only two embedding models are supported
- **Where:** `crates/lucidos-engine/src/memory/fastembed.rs` (`resolve_model` errors on anything but `bge-small-en-v1.5` / `multilingual-e5-small`)
- **Gap:** `LUCIDOS_EMBEDDING_MODEL` accepts only those two 384-dim models; any other id is rejected at startup.
- **Why:** The resolver hardcodes the two supported models (schema/backfill assume 384-dim vectors).
- **Status:** Open — no alternate embedding model without code + a migration for the vector dimension.

### Memory created during the model-load window isn't auto-indexed
- **Where:** `crates/lucidos-engine/src/engine/memory/extract.rs` (`index_memory_inner_impl` skips when `!self.embedder.is_ready()`), `crates/lucidos-engine/src/engine/memory/embedder_retry.rs` (post-install `reembed_stale`)
- **Gap:** The embedding model loads in the background so boot is never blocked (see `memory::EmbedderSlot`); chat messages / artifacts created before it lands are not extracted into memory, and are not indexed automatically once it does.
- **Why:** Indexing requires an embedding, which errors `EMBEDDER_UNAVAILABLE` while the slot is empty. The post-install `reembed_stale` sweep only re-embeds *existing* `memory_entries` rows (stale model id) — it can't create rows for items that never inserted one. On a warm cache the window is a few seconds (extraction runs post-response, so the model is almost always ready in time); on a cold cache it lasts the ~465 MB download.
- **Status:** Open — a manual memory rebuild **run once memory is active** recovers boot-window items (a rebuild is refused while the model is still loading, since it clears entries before re-indexing — see `rebuild_memory`); a durable replay queue was out of scope for the boot-unblock change (`docs/plans/2026-07-07-background-embedding-model-load.md`).

## Coding agents

### A coding-agent thread's backend is locked at first session
- **Where:** `crates/lucidos-engine/src/api/chat.rs` (`validate_thread_continuity` → `StatusCode::CONFLICT`), `docs/adr/0004-codex-as-second-coding-agent.md`
- **Gap:** You cannot switch a thread between Claude Code and Codex after it starts — a follow-up requesting the other backend is rejected with HTTP 409.
- **Why:** The backend is chosen at first send and locked; the other backend has no session to resume, so a mid-thread flip would silently lose the conversation context.
- **Status:** Open by design — start a new thread to use the other backend.

### The Codex `exec` protocol lacks cards, streaming, and graceful interrupt
- **Where:** `crates/lucidos-engine/src/runtime/codex.rs` (module header), `docs/adr/0005-codex-app-server-protocol.md`, `crates/lucidos-engine/src/engine/agent_session/run_session/run.rs` (`INTERRUPT_ESCALATE_AFTER`)
- **Gap:** Running Codex under `LUCIDOS_CODEX_PROTOCOL=exec` (the escape hatch) gives no permission cards (sandbox escalations fail instead of asking), no per-token streaming (items arrive whole), and no graceful interrupt (cancel kills the child, losing partial work).
- **Why:** `codex exec` runs exactly one turn per process with no protocol for permission requests or a graceful wind-down; the OS sandbox is the only guard. It's kept wired only as a rollback lever because the app-server protocol is upstream-experimental. (Even app-server's graceful `turn/interrupt` hard-kills after an 8s escalation.)
- **Status:** Open by design — default `app-server` protocol carries these; `exec` is a deliberately reduced fallback.

### Cross-workspace coding-agent spawns can't be child threads
- **Where:** `crates/lucidos-engine/src/llm/tools/threads.rs` (`run_coding_agent` — "child-thread auto-resume callbacks across workspaces are unsupported; the tool refuses child + cross-workspace with an error")
- **Gap:** Spawning a coding agent into a *different* workspace must be `relation="top"`; you can't get the automatic callback-resume of a child thread across workspaces.
- **Why:** The auto-resume callback that a child thread relies on isn't wired to cross a workspace boundary.
- **Status:** Open — cross-workspace work runs as an independent top-level thread; sequential dependencies need manual follow-up.

### `run_coding_agent` can't target unregistered or non-git folders
- **Where:** `crates/lucidos-engine/src/llm/tools/threads.rs` (`run_coding_agent` description — unregistered git folder / non-git directory "not supported in v1; refused with a clear error")
- **Gap:** You can't point a coding-agent session at an arbitrary directory — only Lucidos itself, an installed app's `data/apps/<id>/`, or a registered external repo.
- **Why:** v1 resolves targets via the repo registry / app layout; an unregistered or non-git path is refused rather than silently handled.
- **Status:** Open — register the repo first (`manage_repositories action='add'`), or use the chat file tools for `data/` subtrees.

## Backup & restore

### Restore is picker-only and local-`.enc`-only
- **Where:** `docs/adr/0015-restore-in-the-workspace-picker.md`, `system-knowhow/backups.md` ("Restore is **not** an engine operation")
- **Gap:** A user/agent can't restore a backup through the engine or a chat action, and can't restore directly from a cloud provider — restore happens only from the workspace picker, from a local encrypted `.enc` file.
- **Why:** The engine is stateless and the gateway (which drives restore) has no per-workspace OAuth (provider tokens live in the workspace's Postgres), so a local file sidesteps cloud auth; restore requires provisioning a fresh workspace, a launcher/gateway responsibility.
- **Status:** Open — download the `.enc` from your provider and restore it via the picker; Settings keeps backup *creation* only.

### Restore drops the archive's `~/.lucidos` user-global content
- **Where:** `docs/adr/0015-restore-in-the-workspace-picker.md` ("Given up (for now): … restoring the archive's `~/.lucidos` content"; `restore_archive_into` drops `user_dir/`)
- **Gap:** Restoring a backup recovers the workspace directory + database but not machine-global content captured in the archive (user-global knowhow, cache, gateway registry).
- **Why:** Restoring `user_dir/` would clobber machine-global state such as the gateway's `workspaces.json`, risking data loss; selective restore was deemed too ambiguous/risky for now.
- **Status:** Open — only workspace dir + DB are restored; `~/.lucidos` is left as-is.

## Agent safety & capability surface

### The chat command guard's middle-lane LLM judge is fallible
- **Where:** `docs/adr/0002-lucidos-agent-command-safety.md` § Consequences ("The LLM-judge is the weakest link and is fallible")
- **Gap:** The `run_bash`/`run_python` safety gate can both over-gate a safe command (friction) and, more importantly, miss a novel side-effect (a genuinely dangerous command slips through).
- **Why:** Safe and catastrophic cases are deterministic, but the ambiguous middle is decided by an LLM judge, and "is this a real side-effect?" is genuinely ambiguous — so false positives and false negatives are inherent.
- **Status:** Open by design — accepted trade-off; the reopen criterion is a too-high judge error rate (fall back to static-only or move inline).

### The command guard does not see through every shell-wrapper or substitution form
- **Where:** `crates/lucidos-engine/src/engine/command_guard.rs` (`unwrap_shell_command`, `catastrophic_reason`, `bash_destruction_scope`, `segment_heads`)
- **Gap:** Three forms still resolve to a head that hides what actually runs, so the command can settle `Safe` (no card on the chat lane with the judge off) or map to `RequestVerdict::Benign` and auto-allow on the unattended coding-agent lane:
  1. **A wrapper behind a benign prefix.** `unwrap_shell_command` resolves the FIRST token, so `sudo bash -c '...'` and `env bash -c '...'` are not unwrapped. The decorated forms (`\bash`, `"bash"`) are, since 2026-08-06.
  2. **Command substitution.** `echo $(rm -rf /)` and the backtick form read as head `echo`. `segment_is_safe` refuses to settle them, but `fallback_classify` then reads the same head and lands on `Safe`.
  3. **`segment_heads` uses a raw head**, so the stored allow pattern for `"rm" -rf x` is `Bash("rm":*)`, which no later `rm` matches.
- **Why:** Each fix is a policy call, not a mechanical one. Command substitution is pervasive in legitimate shell usage (`cd $(git rev-parse --show-toplevel)`), so routing all of it to a block, or even always to the judge, has a real over-blocking cost. And `segment_heads` feeds both the stored pattern and the grant match, so normalizing it would make an existing `Bash(rm:*)` grant start covering `\rm`, which is the one direction that loosens.
- **Status:** Open. Tracked as work items `harden-20260806-command-substitution-settles-safe` and the notes in `docs/code-review-priors.md`. The likely shape for (1) and (2) is to have `catastrophic_reason` and `bash_destruction_scope` recurse into wrapper and substitution bodies the way the segment scan already recurses into `sh -c`, so the inner command is classified on its own merits.

### AskUserQuestion is capped at 4 options and 4 questions
- **Where:** `docs/adr/0017-ask-user-question-four-option-cap.md`, `crates/lucidos-engine/src/llm/tools/misc.rs` (`maxItems: 4`)
- **Gap:** A question asked via the agent's `ask_user_question` tool (and Claude Code's `AskUserQuestion`) can present at most 4 tappable options, and at most 4 questions per call.
- **Why:** Inherited from Claude Code's native `AskUserQuestion` schema (`maxItems: 4`); the mobile-first card fits ~4 buttons without scrolling.
- **Status:** Open — for more than 4 choices, enumerate the overflow in prose and take a free-text answer.

### Archive and cancel carry no parent-child check, so any thread can reach any thread
- **Where:** `crates/lucidos-engine/src/api/thread_reach.rs` (the gate), `crates/lucidos-engine/src/api/threads/archive.rs` (`archive_thread`), `crates/lucidos-engine/src/api/chat.rs` (`cancel_chat`); contrast `docs/adr/0043-parent-to-child-privileged-write.md` and `docs/adr/0083-sibling-threads-observe-never-direct.md`
- **Gap:** `POST /api/v1/threads/archive` and `POST /api/v1/chat/cancel` accept any thread id in the workspace and run no authorization ladder. The actor resolved from headers is display attribution only (ADR 0050). So a coding-agent subprocess reaching the loopback API can archive or cancel a sibling thread, or its own parent.
- **Why:** Both predate the thread-bound origin token. The messaging edge got the ladder when it shipped (`follow_up_child_thread` refuses unless `row.parent_thread_id == caller`), and the two older destructive verbs were never revisited. No LLM tool exposes either, so no agent reaches them by accident, which is why this went unnoticed.
- **Status:** **Closed (2026-08-17)** by `docs/plans/2026-08-17-archive-and-cancel-reach-self-and-descendants.md`. Both routes now run the ladder in `api::thread_reach`: a token-bearing caller reaches itself and its own descendants, and nothing further, with a 403 otherwise. A caller with no token is untouched: the user's device, the local API surface, the e2e suites. A threadless subprocess is refused rather than handed every thread, and a thread-bound caller cannot cancel everything by omitting `thread_id`. **One residual, and it belongs to the whole surface:** `/api/v1` has no authentication (*unattributed caller* in `docs/glossary.md`). So a subprocess that drops its token keeps full reach, and closing that is owed its own ADR.

### Not every HTTP route is agent-callable
- **Where:** `docs/adr/0018-capability-parity-manifest.md` (declared parity, not blanket parity), `crates/lucidos-engine/src/capability_manifest/`
- **Gap:** Only capabilities declared in the capability-parity manifest are surfaced as LLM tools / CLI commands / SDK methods; many HTTP routes (backup schedules, disk usage, device registration, blobs, compose drafts, …) are UI/infra-only and not agent-reachable.
- **Why:** A blanket "every route everywhere" rule would generate nonsense tools; the manifest declares per-capability which surfaces get it, enforced by codegen (build fails on drift).
- **Status:** Open by design — add a capability to the manifest to expose it across surfaces.
