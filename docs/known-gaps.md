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
- **Where:** `.claude/rules/scripts.md` § "Gateway self-reload"; gateway control status `packaged` field (`crates/lucidos-gateway/src/`), `crates/lucidos-app/src/desktop.rs` (`LUCIDOS_PACKAGED=1`), `crates/lucidos-app/src/components/.../WorkspacePicker.tsx`
- **Gap:** The workspace picker's "reload the gateway onto the rebuilt binary" control (`POST /~/api/v1/control/gateway/reload`, re-exec in place) is hidden in packaged builds.
- **Why:** In-place re-exec onto a rebuilt on-disk binary only makes sense in dev (a CC Apply rebuilds the gateway under a running gateway). A packaged build never rebuilds in place — its updates go through the app updater + a full launchd service restart.
- **Status:** Open by design — packaged gateway updates are delivered by the updater, not an in-place reload.

### Frontend-only in-process Apply refresh is dev-only
- **Where:** `crates/lucidos-engine/src/engine/frontend_refresh.rs` (`refresh_served_frontend_after_rebuild` and the peer `spawn_served_frontend_sync` both early-return packaged/headless), `.claude/rules/scripts.md` § "Exception — a frontend-only Apply…" and § "Shared build-watch"
- **Gap:** The fast path where a pure frontend change advances the served client without an engine restart exists only in dev — both for the applying workspace (in-process re-snapshot on Apply) and for **peer** workspaces (a ~10s periodic re-snapshot of the checkout-shared `dist/` when INV-A-safe, emitting `ServedFrontendAdvanced`); packaged Applies don't get an in-process client swap.
- **Why:** In dev the engine serves a pinned snapshot of a live `dist/` and can re-snapshot in place. Packaged serves an immutable bundled Resources directory that is already one unit — there is no live `dist/` to re-snapshot, so the refresh is a no-op there. (Packaged is also single-workspace-per-install, so the cross-workspace peer case doesn't arise.)
- **Status:** Open by design — packaged frontend updates ship via the updater's full restart. The cross-workspace peer propagation gap (a peer serving a stale client with no badge) is **closed in dev** by the periodic sync — see `docs/plans/2026-07-03-cross-workspace-frontend-only-refresh.md`.

### No HMR anywhere; no Vite in the serving path
- **Where:** `crates/lucidos-app/src/main.tsx` (`import.meta.hot` path inert under built serving), `.claude/rules/scripts.md` § "Frontend: the engine serves the built `dist/`", ADR 0014
- **Gap:** There is no hot-module-replacement in the running app; a code change is picked up by a full browser refresh after the build-watch republishes `dist/`.
- **Why:** ADR 0014 removed the live Vite dev server from the serving path — the engine serves the built `dist/` directly. The build-watch runs a fresh `vite build` per change instead of incremental HMR; the old `import.meta.hot` path is inert under built serving.
- **Status:** Open by design — reload to pick up a change (rebuild is typically sub-second).

### The default `curl | sh` installer 404s today
- **Where:** `install.sh` (download-and-run default), `.github/workflows/release-tarballs.yml` (artifact-only, `attach_to_release` gated off), `.claude/rules/scripts.md` § Installer "Caveat"
- **Gap:** The default installer path downloads `lucidos-<version>-<triple>.tar.gz` from GitHub Releases, but no release asset is published yet, so the download returns 404.
- **Why:** The release workflow uploads tarballs as **workflow artifacts only** and never auto-creates a Release; attaching to a Release is behind a manual `workflow_dispatch` + tag ref + opt-in flag. Nothing has been published.
- **Status:** Open — use `install.sh --dev` (build from source) or `install.sh --from-tarball <path>` until releases are published; the failure message points at these.

### Linux / cross-arch releases come only from the CI matrix
- **Where:** `scripts/build-headless.sh` (native-only; `--triple` must equal the host), `.github/workflows/release-tarballs.yml` (per-arch runners, `ubuntu:22.04` container, "Assert portability floor")
- **Gap:** You cannot locally produce a Linux (or cross-architecture) release tarball for a triple other than the build host's, and the resulting binaries won't start on distros older than the glibc 2.35 floor (pre Ubuntu 22.04 / Debian 12 / RHEL 9).
- **Why:** `build-headless.sh` compiles natively (relocatable Postgres + pgvector are fetched/compiled per platform), so cross-arch artifacts must come from CI's per-arch native runners; the Linux entries build inside `ubuntu:22.04` to pin the glibc floor, and a symbol above it fails the build.
- **Status:** Open — non-host Linux/arch users wait for CI artifacts or use `--dev` (build from source on the target). Older-glibc distros are unsupported by the prebuilt tarballs.

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

### AskUserQuestion is capped at 4 options and 4 questions
- **Where:** `docs/adr/0017-ask-user-question-four-option-cap.md`, `crates/lucidos-engine/src/llm/tools/misc.rs` (`maxItems: 4`)
- **Gap:** A question asked via the agent's `ask_user_question` tool (and Claude Code's `AskUserQuestion`) can present at most 4 tappable options, and at most 4 questions per call.
- **Why:** Inherited from Claude Code's native `AskUserQuestion` schema (`maxItems: 4`); the mobile-first card fits ~4 buttons without scrolling.
- **Status:** Open — for more than 4 choices, enumerate the overflow in prose and take a free-text answer.

### Not every HTTP route is agent-callable
- **Where:** `docs/adr/0018-capability-parity-manifest.md` (declared parity, not blanket parity), `crates/lucidos-engine/src/capability_manifest/`
- **Gap:** Only capabilities declared in the capability-parity manifest are surfaced as LLM tools / CLI commands / SDK methods; many HTTP routes (backup schedules, disk usage, device registration, blobs, compose drafts, …) are UI/infra-only and not agent-reachable.
- **Why:** A blanket "every route everywhere" rule would generate nonsense tools; the manifest declares per-capability which surfaces get it, enforced by codegen (build fails on drift).
- **Status:** Open by design — add a capability to the manifest to expose it across surfaces.
