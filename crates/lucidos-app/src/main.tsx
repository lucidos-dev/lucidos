// MUST be first: installs per-workspace localStorage namespacing before any
// module-init localStorage read (e.g. store/store.ts) — see the module's docs.
import './utils/workspaceStorage.install';
import { render } from 'preact';
import { App } from './App';
import { lazyComponent } from './utils/lazyComponent';
import { IS_PICKER, WORKSPACE_ID, baseContextIsValid } from './utils/basePath';
import { rememberLastWorkspace } from './utils/lastWorkspace';
import { updateAvailable } from './store/store';
import { installActionBtnBlurListener } from './components/chat/promptFocus';
import { installNoAutofill } from './utils/noAutofill';
import { installNoDrag } from './utils/noDrag';
import { publishScrollbarGutter } from './utils/scrollbarGutter';
import { isTouchDevice } from './utils/viewport';
import { isIOSPwa, isTauri, isTauriPreGatewayEntry } from './utils/platform';
import { invoke } from './utils/tauri';
import { setBootStatus } from './utils/bootSplash';
import { startStartupStatusPolling } from './utils/startupStatus';
import { reconcileDesktopDeviceId } from './store/actions/devices';
import { openAppById } from './store/actions/apps';
import { startPerfProbe } from './utils/perfProbe';
import './styles/global.css';
import './styles/picker.css';
import './styles/header.css';
import './styles/panels.css';
import './styles/chat.css';
import './styles/steps.css';
import './styles/settings.css';
import './styles/components.css';
import './styles/pages.css';
import './styles/skills.css';
import './styles/thread-queue.css';
import './styles/mobile.css';
import './styles/drawer.css';
import './store/effects';
import './store/actions/wipPreview';

// The inline pre-hydration watchdog owns the failure case where this module
// graph never evaluates (for example, a stale hashed entry bundle). Handing over
// tells it the graph is live: it clears the 15s stall timer AND the entry-script
// error listener, so from that call on, a boot that dies is the application's to
// recover. Also clears the watchdog's guarded retry state.
function handOverBootOwnership(): void {
  (window as Window & { __lucidosBootLoaded?: () => void }).__lucidosBootLoaded?.();
}

// The app path hands over here, because reaching this line really does mean its
// whole root is in memory. The PICKER path does NOT: its root arrives in a
// second chunk (see WorkspacePicker below), so handing over here would disarm
// the watchdog while the thing it guards is still in flight. That matters
// because the watchdog is the ONLY recovery a picker document has: the gateway
// escape link is deliberately offered to direct-port documents alone (see
// revealGatewayEscape in index.html), leaving the picker with the inline
// retry-once + tap-to-retry, which is also the only way back for an iOS PWA
// with no reload button. `lazyComponent`'s own stale-chunk reload does not
// substitute for it: it stands down whenever sessionStorage is unavailable or
// it already reloaded in the last 30s, and its fallback toast cannot render on
// a document whose only component is the chunk that failed. So the picker hands
// over once its chunk resolves, and a chunk that never arrives falls back to
// exactly the recovery a static import used to get. The two paths cannot both
// fire: `isTauriPreGatewayEntry()` (the one boot() path that renders nothing)
// is false whenever IS_PICKER is true, since the picker context is a
// gateway-stamped `<base href="/~/">` and the pre-gateway shell has no base.
if (!IS_PICKER) handOverBootOwnership();

if (isTouchDevice()) {
  document.body.classList.add('is-touch');
}

// iPhone OLED panels emit saturated deep blues with a violet tint, so the
// blue header chrome reads slightly purple in the iOS PWA while looking correct
// on desktop. Tag the root so base.css can nudge the dark-mode brand blues
// toward cyan for this context only — desktop/Android stay pixel-identical.
if (isIOSPwa()) {
  document.documentElement.classList.add('ios-pwa');
}

installActionBtnBlurListener();
// Suppress WebKit's saved-value autofill dropdown (+ its white→dark flash) and
// autocorrect/autocapitalize on every text field — App and Picker both. See
// utils/noAutofill.ts.
installNoAutofill();
// Tauri desktop only: stop WebKit's native content drag (the green-"+" copy
// badge + translucent text/image ghost) that otherwise flashes while dragging
// the window by the header. See utils/noDrag.ts. No-op off Tauri.
installNoDrag();
// Publish --scrollbar-gutter-width before the first render: the chat composer
// sizes its horizontal inset off it to stay aligned with the transcript, whose
// scrollbar takes that width out of the content box on classic-scrollbar
// platforms. The inline FOUC script has already applied the UI scale, so the
// rem-sized scrollbar measures at its final width here.
//
// Nothing is mounted yet, so this is the probe's estimate. ThreadView re-publishes
// from the real transcript the moment one exists, which is the only answer that
// holds on every engine (utils/scrollbarGutter.ts).
publishScrollbarGutter();

// E2E test hook — Playwright opens an app by id from `page.evaluate`. The
// real `openApp(app: App)` requires an `App` object that the test doesn't
// hold, and `openAppById` lives behind an ES import the browser can't reach
// from `page.evaluate`. There is no `#app=<id>` hash route to fall back on
// (the hash router only handles `#thread=` / `#notification=`), so this
// window hook is the ONLY way the e2e suite can open an app from outside the
// bundle.
//
// Installed unconditionally. The hook MUST be in the e2e bundle, which since
// ADR 0014 is a production `vite build` (the engine serves a fixed `dist/`;
// there is no Vite dev server) — the previous `MODE !== 'production'` gate
// silently stripped it from every e2e build, leaving app-open specs to time
// out waiting for an iframe that never mounted. e2e also reuses a
// checkout-shared `dist/` that may have been built by any path, so no
// build-time flag reliably scopes the hook to e2e. Shipping it everywhere is
// safe: `openAppById` only opens an already-installed app in the panel
// overlay — a normal user action that carries no extra privilege.
(window as unknown as { __openApp?: (id: string) => Promise<void> }).__openApp =
  openAppById;

// Decide the root component from the server-stamped `<base href>` (ADR 0014).
// The gateway serves the picker context with `<base href="/~/">`; a workspace
// (proxied) or a legacy direct engine gets `/<slug>/` or `/`. So `IS_PICKER`
// decides synchronously — no probe of the control plane is needed, and the
// old root-path ambiguity (gateway picker vs. legacy engine, both at `/`) is
// gone because they now carry different base hrefs.
const appRoot = document.getElementById('app')!;

// The picker and the app are MUTUALLY EXCLUSIVE render roots (see the render
// call in `boot()`), so shipping the picker inside the eager entry chunk sent
// every workspace document ~43 kB it could never mount. Code-split it: the app
// path stops paying for it entirely, and the picker path fetches it while the
// inline boot splash is still covering the screen. `lazyComponent` renders null
// until the chunk lands, so the splash simply stays up one round trip longer,
// and its stale-chunk arm (reload to the new build) already covers the failure
// a hashed-URL 404 would otherwise strand the picker on. `<App/>` deliberately
// stays static: it IS the eager graph (main.tsx pulls the store, the effects
// and the actions in on its own), so splitting it would move bytes into a
// second chunk without removing any from the critical path.
const WorkspacePicker = lazyComponent(() =>
  import('./components/picker/WorkspacePicker').then((m) => {
    // The picker's root is here now, so this is where the picker path proves
    // its graph is live. See handOverBootOwnership above for why it waits.
    handOverBootOwnership();
    return m.WorkspacePicker;
  }),
);

// Defensive recovery (boot-recovery plan): a workspace bundle that loaded in a
// malformed base-path context can't build valid URLs from it — every fetch + SW
// registration throws WebKit's "string did not match the expected pattern" and
// the app is a dead-end with no way back to the picker. Bounce ONCE to the
// workspace picker (`/~/?pick`, which also stands the cold-start auto-open
// redirect down) instead of rendering the broken app. One-shot guarded so it
// can't loop (the `?pick` already prevents an auto re-open; this is belt-and-
// suspenders). Returns true when it redirected — render is then skipped.
const RECOVER_REDIRECT_KEY = 'lucidos-recover-redirect';
function recoverFromBrokenContext(): boolean {
  if (WORKSPACE_ID === null || baseContextIsValid()) {
    // Valid context — clear the one-shot so a future genuine failure can redirect.
    try { sessionStorage.removeItem(RECOVER_REDIRECT_KEY); } catch { /* storage off */ }
    return false;
  }
  let alreadyTried = false;
  try { alreadyTried = sessionStorage.getItem(RECOVER_REDIRECT_KEY) === '1'; } catch { /* storage off */ }
  if (alreadyTried) return false; // already bounced once — render rather than loop
  try { sessionStorage.setItem(RECOVER_REDIRECT_KEY, '1'); } catch { /* storage off */ }
  location.replace('/~/?pick');
  return true;
}

/** Packaged desktop shell, BEFORE `desktop::launch()` has navigated the window to
 *  the gateway: the window is on Tauri's bundled asset scheme, where booting
 *  `<App/>` would fire API calls + a service-worker registration that all throw
 *  WebKit's "string did not match the expected pattern". Keep the inline boot
 *  splash up with a "Starting Lucidos…" status; `desktop::launch()` navigates this
 *  window to the gateway once the service is healthy (a full document load that
 *  replaces the splash with the real app). Heartbeat on the cadence `useStartup`
 *  would, so the WKWebView crash watchdog (lib.rs) doesn't reload the splash every
 *  ~60s while we wait. */
function stayOnStartingSplash(): void {
  // The opening line, painted now rather than a poll-tick from now. Matches
  // `desktop::STARTING_LABEL`, which is also what the first poll answers, so a
  // fast start never sees the text change at all.
  setBootStatus('Starting Lucidos…');
  // As in useStartup: the `catch` is a local no-op, and `invoke` reports bridge
  // failures to the engine log on its own (utils/ipcHealth). Note this splash
  // runs on the bundled `tauri://localhost` origin, which the ACL treats as
  // LOCAL — so a heartbeat working here says nothing about whether it will keep
  // working after desktop::launch navigates to the (remote) gateway origin.
  invoke('heartbeat').catch(() => {});
  window.setInterval(() => { invoke('heartbeat').catch(() => {}); }, 15_000);
  // Separate from the heartbeat and deliberately faster: the heartbeat's job is
  // to keep the crash watchdog quiet, and slowing this to its cadence would make
  // the elapsed counter jump in 15s steps. See utils/startupStatus.
  startStartupStatusPolling({
    invoke,
    setStatus: setBootStatus,
    setInterval: (fn, ms) => window.setInterval(fn, ms),
  });
}

async function boot() {
  // Packaged desktop shell before it has navigated to the gateway: stay on the
  // boot splash instead of booting a broken `<App/>` against the asset scheme
  // (see stayOnStartingSplash). There is no workspace context here, so the
  // device-id reconcile + broken-context recovery below would be no-ops anyway —
  // returning early just avoids the invalid API/SW calls.
  if (isTauriPreGatewayEntry()) {
    stayOnStartingSplash();
    return;
  }

  // Desktop only: make the per-workspace device id durable across DMG reinstalls by
  // reconciling it with the native store BEFORE the first API call (the id rides
  // every request as `x-lucidos-device-id`). Browser/PWA skip this — their
  // localStorage id is already durable, and an async function runs synchronously up
  // to its first await, so the non-Tauri render below is unchanged in timing.
  if (isTauri() && !IS_PICKER && WORKSPACE_ID) {
    await reconcileDesktopDeviceId(WORKSPACE_ID);
  }

  if (!recoverFromBrokenContext()) {
    // Remember the workspace the user is in, so the gateway's smart root (`/`) can
    // auto-open it next time (see lastWorkspace.ts / WorkspacePicker). Only inside a
    // real workspace — never the picker (IS_PICKER) or legacy direct-engine root
    // (WORKSPACE_ID null).
    if (!IS_PICKER && WORKSPACE_ID) rememberLastWorkspace(WORKSPACE_ID);
    // Permanent perf debug tooling: surfaces the main-thread blocker behind any
    // interaction lag. Quiet unless an interaction/task crosses the threshold;
    // logs to the browser console only. See utils/perfProbe.ts.
    if (!IS_PICKER) startPerfProbe();
    render(IS_PICKER ? <WorkspacePicker /> : <App />, appRoot);
  }
}
void boot();

if (import.meta.hot) {
  import.meta.hot.accept();

  // Server-side suppressMergeReload plugin detects git-merge file bursts and
  // drops the HMR update before it reaches the client. It sends this custom
  // event so the client can show an "update available" indicator.
  import.meta.hot.on('lucidos:update-available', () => {
    updateAvailable.value = true;
  });

  // Always suppress Vite full-reloads — show an "update available" indicator
  // instead. Full-reloads lose UI state and, because all Vite instances share
  // the same source directory, a rebuild in one workspace triggers reloads in
  // every workspace's browser tab. Suppressing unconditionally keeps all tabs
  // stable; the user reloads manually via the refresh button when ready.
  import.meta.hot.on('vite:beforeFullReload', () => {
    updateAvailable.value = true;
    throw 'suppress-reload';
  });

  // Suppress Vite's auto-reload on WebSocket disconnect. When the engine
  // restarts, the Vite dev server also restarts. Vite's client polls until
  // Vite comes back, then calls location.reload() directly (bypassing
  // vite:beforeFullReload). This reload hits the engine port before the
  // engine is ready, causing a "Can't connect to localhost" browser error.
  // Throwing here prevents that reload — the health-check polling in
  // connection.ts handles reconnection gracefully with the red status dot.
  import.meta.hot.on('vite:ws:disconnect', () => {
    throw 'suppress-reload';
  });
}
