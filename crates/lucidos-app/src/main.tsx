// MUST be first: installs per-workspace localStorage namespacing before any
// module-init localStorage read (e.g. store/store.ts) — see the module's docs.
import './utils/workspaceStorage.install';
import { render } from 'preact';
import { App } from './App';
import { WorkspacePicker } from './components/picker/WorkspacePicker';
import { IS_PICKER, WORKSPACE_ID, baseContextIsValid } from './utils/basePath';
import { rememberLastWorkspace } from './utils/lastWorkspace';
import { updateAvailable } from './store/store';
import { installActionBtnBlurListener } from './components/chat/promptFocus';
import { installNoAutofill } from './utils/noAutofill';
import { installNoDrag } from './utils/noDrag';
import { isTouchDevice } from './utils/viewport';
import { isIOSPwa } from './utils/platform';
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

if (!recoverFromBrokenContext()) {
  // Remember the workspace the user is in, so the gateway's smart root (`/`) can
  // auto-open it next time (see lastWorkspace.ts / WorkspacePicker). Only inside a
  // real workspace — never the picker (IS_PICKER) or legacy direct-engine root
  // (WORKSPACE_ID null).
  if (!IS_PICKER && WORKSPACE_ID) rememberLastWorkspace(WORKSPACE_ID);
  // TEMP: locate the click-lag main-thread blocker (dev workspace). Quiet unless
  // an interaction/task crosses the threshold. Remove once measured.
  if (!IS_PICKER) startPerfProbe();
  render(IS_PICKER ? <WorkspacePicker /> : <App />, appRoot);
}

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
