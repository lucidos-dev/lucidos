// MUST be first: installs per-workspace localStorage namespacing before any
// module-init localStorage read (for example store/store.ts).
import './utils/workspaceStorage.install';
import { render, type ComponentChildren } from 'preact';
import { App } from './App';
import { lazyComponent } from './utils/lazyComponent';
import { IS_PICKER, WORKSPACE_ID, baseContextIsValid } from './utils/basePath';
import { rememberLastWorkspace } from './utils/lastWorkspace';
import { updateAvailable } from './store/store';
import { installActionBtnBlurListener } from './components/chat/promptFocus';
import { installNoAutofill } from './utils/noAutofill';
import { installNoDrag } from './utils/noDrag';
import { installNoFunctionKeyText } from './utils/noFunctionKeyText';
import { installStrayFileDropGuard } from './utils/strayFileDrop';
import { publishScrollbarGutter } from './utils/scrollbarGutter';
import { isTouchDevice } from './utils/viewport';
import { isIOSPwa, isTauri, isTauriPreGatewayEntry } from './utils/platform';
import { invoke, windowReadyToShow } from './utils/tauri';
import { handOverBootOwnership, setBootStatus } from './utils/bootSplash';
import { startStartupStatusPolling } from './utils/startupStatus';
import { adoptGatewayDeviceId, reconcileDesktopDeviceId } from './store/actions/devices';
import { adoptDeviceIdFromUrl } from './utils/deviceIdSeed';
import { takePairingCodeFromUrl } from './utils/pairingCodeSeed';
import { openAppById } from './store/actions/apps';
import { startPerfProbe } from './utils/perfProbe';
import './styles/global.css';
import './styles/picker.css';
import './styles/header.css';
import './styles/header-mark.css';
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

// The app path hands over here: reaching this line means its whole root is in
// memory. The PICKER path must NOT, since its root arrives in later chunks
// (see PairingGate and WorkspacePicker below). Handing over here would disarm
// the watchdog while the thing it guards is still in flight, and that watchdog
// is the picker's only recovery.
//
// The gateway escape link is offered to direct-port documents alone
// (revealGatewayEscape in index.html), and `lazyComponent`'s stale-chunk reload
// stands down whenever sessionStorage is unavailable. So the picker hands over
// once its chunk resolves. The two paths cannot both fire: the picker context
// is a gateway-stamped `<base href="/~/">`, and the pre-gateway shell has no
// base at all.
if (!IS_PICKER) handOverBootOwnership();

if (isTouchDevice()) {
  document.body.classList.add('is-touch');
}

// iPhone OLED panels tint saturated deep blues violet, so the blue header
// chrome reads slightly purple in the iOS PWA. Tag the root so base.css can
// nudge the dark-mode brand blues toward cyan here only, leaving desktop and
// Android pixel-identical.
if (isIOSPwa()) {
  document.documentElement.classList.add('ios-pwa');
}

installActionBtnBlurListener();
installNoAutofill();
installNoDrag();
installNoFunctionKeyText();
// Both render roots, so a near-miss drop cannot navigate the picker document
// either (utils/strayFileDrop.ts).
installStrayFileDropGuard();
// Publish --scrollbar-gutter-width before the first render: the chat composer
// sizes its horizontal inset off it to stay aligned with the transcript.
// Nothing is mounted yet, so this is the probe's estimate; ThreadView
// re-publishes from the real transcript once one exists
// (utils/scrollbarGutter.ts).
publishScrollbarGutter();

// E2E test hook: Playwright opens an app by id from `page.evaluate`. The real
// `openApp(app: App)` needs an `App` object the test does not hold, and
// `openAppById` sits behind an ES import `page.evaluate` cannot reach. There is
// no `#app=<id>` hash route either, so this window hook is the suite's only way
// to open an app from outside the bundle.
//
// Installed unconditionally, and it MUST stay that way: the e2e bundle is a
// production `vite build` (ADR 0014) over a checkout-shared `dist/`, so no
// build-time flag scopes the hook to e2e. Shipping it everywhere is safe, since
// `openAppById` only opens an installed app in the panel overlay.
(window as unknown as { __openApp?: (id: string) => Promise<void> }).__openApp =
  openAppById;

// `IS_PICKER` picks the root component from the server-stamped `<base href>`
// (ADR 0014): the gateway serves the picker context as `<base href="/~/">`,
// while a proxied workspace or a legacy direct engine gets `/<slug>/` or `/`.
// So the choice resolves synchronously, with no probe of the control plane.
const appRoot = document.getElementById('app')!;

// The picker and the app are MUTUALLY EXCLUSIVE render roots. Shipping the
// picker in the eager entry chunk sent every workspace document ~43 kB it could
// never mount. Code-split, the picker path fetches it while the inline boot
// splash is still up. `lazyComponent`'s stale-chunk arm covers the hashed-URL
// 404 that would otherwise strand it. `<App/>` deliberately stays static: it IS
// the eager graph, so splitting it would move bytes into a second chunk without
// shortening the critical path.
//
// The gate splits for the same reason: it renders only under `IS_PICKER`, so a
// workspace document carried a screen it could never mount. The picker now
// fetches two chunks in series, because the gate holds its children back until
// the session probe answers. Both land under the boot splash.
//
// The gate must NOT hand boot ownership over when its chunk resolves, the way
// the picker below does. The watchdog stays armed across the session probe.
// `PairingGate` hands over itself once it finds this device unpaired, and the
// picker chunk hands over on resolve. Both are after the probe.
const PairingGate = lazyComponent<{ children: ComponentChildren }>(() =>
  import('./components/picker/PairingGate').then((m) => m.PairingGate),
);

const WorkspacePicker = lazyComponent(() =>
  import('./components/picker/WorkspacePicker').then((m) => {
    // The picker path proves its graph is live here, not at the eager
    // hand-over above.
    handOverBootOwnership();
    return m.WorkspacePicker;
  }),
);

// A workspace bundle loaded in a malformed base-path context cannot build valid
// URLs from it: every fetch and SW registration throws WebKit's "string did not
// match the expected pattern", leaving no way back to the picker. Bounce ONCE
// to `/~/?pick` instead of rendering the broken app. The one-shot guard is what
// keeps that bounce from looping. Returns true when it redirected, and render
// is then skipped.
const RECOVER_REDIRECT_KEY = 'lucidos-recover-redirect';
function recoverFromBrokenContext(): boolean {
  if (WORKSPACE_ID === null || baseContextIsValid()) {
    // Valid context: clear the one-shot so a future genuine failure can redirect.
    try { sessionStorage.removeItem(RECOVER_REDIRECT_KEY); } catch { /* storage off */ }
    return false;
  }
  let alreadyTried = false;
  try { alreadyTried = sessionStorage.getItem(RECOVER_REDIRECT_KEY) === '1'; } catch { /* storage off */ }
  if (alreadyTried) return false; // already bounced once, so render rather than loop
  try { sessionStorage.setItem(RECOVER_REDIRECT_KEY, '1'); } catch { /* storage off */ }
  location.replace('/~/?pick');
  return true;
}

/** Packaged desktop shell, BEFORE `desktop::launch()` has navigated to the
 *  gateway. The window is on Tauri's bundled asset scheme, where booting
 *  `<App/>` would fire API calls and a service-worker registration. Both throw
 *  WebKit's "string did not match the expected pattern". Keep the inline boot
 *  splash up instead; `desktop::launch()` navigates to the gateway once the
 *  service is healthy. Heartbeat on the cadence `useStartup` would, so the
 *  WKWebView crash watchdog (lib.rs) does not reload the splash while we wait. */
function stayOnStartingSplash(): void {
  // Painted now rather than a poll-tick from now, and matching
  // `desktop::STARTING_LABEL`, so a fast start never sees the text change.
  setBootStatus('Starting Lucidos…');
  // As in useStartup: the `catch` is a local no-op, and `invoke` reports bridge
  // failures to the engine log itself (utils/ipcHealth). This splash runs on the
  // bundled `tauri://localhost` origin, which the ACL treats as LOCAL. A
  // heartbeat working here says nothing about the gateway origin (ADR 0028).
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
  // Frontend preview only (ADR 0055): adopt the `?device-id=` the preview link
  // carried, BEFORE the first API call or preference read. That resolves the
  // same device-scoped preferences instead of registering a new device, and it
  // is a no-op everywhere else.
  adoptDeviceIdFromUrl();

  // A scanned pairing QR arrives as `?pair=<code>`. Read and strip it here, so
  // the code never survives into a reload, a bookmark or an installed PWA's
  // start URL. `PairingGate` asks the same memoized function for the value.
  takePairingCodeFromUrl();

  // Packaged desktop: the shell keeps the launch window hidden until a page says
  // it has something to paint (lib.rs `window_ready_to_show`). Signal here
  // rather than from `applyTheme`, because EVERY boot path reaches this line
  // with the theme resolved and the boot splash in the markup. Neither launch
  // that matters reaches `applyTheme`: the pre-gateway shell below returns
  // before `<App/>` mounts, and `loadPreferences` skips the repaint when the
  // stored theme is unchanged.
  if (isTauri()) windowReadyToShow();

  // Packaged desktop shell before it reaches the gateway: stay on the boot
  // splash instead of booting a broken `<App/>` against the asset scheme (see
  // stayOnStartingSplash). There is no workspace context here, so returning
  // early only avoids the invalid API and SW calls.
  if (isTauriPreGatewayEntry()) {
    stayOnStartingSplash();
    return;
  }

  // Behind the gateway, the device id is the one it authenticated us as, not
  // one we minted. Adopt it BEFORE the first API call and before registration,
  // so the row this workspace keeps belongs to the device that paired. Also
  // hands the old row over, in the one window where both ids are known.
  //
  // This is the FIRST await on the boot path, and unlike the reconcile below it
  // is not desktop-only: browser and PWA pay it too. It is bounded for exactly
  // that reason. A gateway that accepts the connection and then stalls would
  // otherwise hold the boot splash with no ceiling. Adopting late costs only a
  // page load.
  const namedByGateway = IS_PICKER
    ? false
    : await adoptGatewayDeviceId(WORKSPACE_ID ?? null);

  // Desktop with no gateway answering: reconcile the per-workspace device id
  // with the native store instead, so it survives a DMG reinstall. Skipped when
  // the gateway named us, because that call would put the stored id back and
  // undo the adoption above.
  if (isTauri() && !IS_PICKER && WORKSPACE_ID && !namedByGateway) {
    await reconcileDesktopDeviceId(WORKSPACE_ID);
  }

  if (!recoverFromBrokenContext()) {
    // Remember the workspace so the gateway's smart root (`/`) can auto-open it
    // next time (see lastWorkspace.ts).
    if (!IS_PICKER && WORKSPACE_ID) rememberLastWorkspace(WORKSPACE_ID);
    // Permanent, not dev-only, and quiet until something crosses a threshold
    // (utils/perfProbe.ts).
    if (!IS_PICKER) startPerfProbe();
    render(
      IS_PICKER ? (
        // The picker shell is the one surface the gateway serves without a
        // credential, so it is where an unpaired device lands and pairs.
        <PairingGate>
          <WorkspacePicker />
        </PairingGate>
      ) : (
        <App />
      ),
      appRoot,
    );
  }
}
void boot();

if (import.meta.hot) {
  import.meta.hot.accept();

  // The server-side suppressMergeReload plugin drops the HMR update on a
  // git-merge file burst, and sends this event in its place.
  import.meta.hot.on('lucidos:update-available', () => {
    updateAvailable.value = true;
  });

  // Always suppress Vite full-reloads, showing an "update available" indicator
  // instead. A full reload loses UI state, and every Vite instance shares one
  // source directory, so a rebuild in one workspace reloads every workspace's
  // tab. The user reloads manually from the refresh button instead.
  import.meta.hot.on('vite:beforeFullReload', () => {
    updateAvailable.value = true;
    throw 'suppress-reload';
  });

  // Suppress Vite's auto-reload on WebSocket disconnect. Vite's client polls
  // until the dev server returns, then calls location.reload() directly,
  // bypassing vite:beforeFullReload. That reload hits the engine port before
  // the engine is ready. Throwing here leaves reconnection to connection.ts.
  import.meta.hot.on('vite:ws:disconnect', () => {
    throw 'suppress-reload';
  });
}
