import {
  appsList,
  currentApp,
  currentAppFragment,
  panelOverlay,
  closeInlineForm,
  pendingChatMessage,
  showToast,
  showConfirm,
  appPseudoFullscreen,
  appRefreshKey,
  wipPreviewThreadId,
  threadMap,
  appSearchOpen,
  appSearchQuery,
  setFocusedThread,
} from '../store';
import { nativeFullscreenElement } from '../appFullscreenHost';
import { clearWipIfMatches } from './wipPreview';
import { toFailed, setLoadingIfFresh } from '../types';
import type { App } from '../types';
import { revealContentPane } from './pane';
import {
  ApiError,
  listAppsApi,
  deleteAppApi,
  updateAppApi,
  stagePluginUninstall,
  appUrl,
  postAppCapture,
  retryTransientRead,
} from '../../api/client';
import { pushNavState, replaceNavState } from './navigation';
import { setAppFrameHash } from '../../components/apps/iframeNav';
import { openPluginUninstallRequest } from './plugin-uninstall';
import { isElementVisible } from '../../components/chat/scrollState';
import { errorDetail } from '../../utils/errorDetail';
import { openExternal } from '../../utils/tauri';

// Shared in-flight load so concurrent callers await the SAME fetch instead of
// racing duplicate GETs: the compose destination picker's render-path kick-off
// and useStartup's eager load both fire on a cold start. An early `return`
// would resolve immediately while the real fetch is still in flight, so an
// `await loadApps()` caller (openAppById) would then read a still-'loading'
// Loadable and falsely report failure — hence sharing the promise, not skipping.
let appsLoadInFlight: Promise<void> | null = null;

// The app-window restore below is a page-reload re-hydration step: it belongs to
// the FIRST successful loadApps() after a fresh load, never to the SSE-driven
// refreshes that fire all session long (AppCreated / AppUpdated / AppDeleted /
// PluginInstalled all call loadApps). Without this one-shot gate, any such
// refresh re-opens the last-opened app and yanks the content pane there,
// clobbering whatever the user is actually looking at. `app-window-open` is
// cleared only by `setActiveMenu` / `closeAppWindow` / nav `restoreState`, so
// opening a file preview, a URL preview or a change diff over an app leaves the
// key set while `currentApp` reads null, which is exactly the condition the
// restore fires on. Mirrors `filePreviewRestoreAttempted` in ./artifacts.ts,
// which fixed the same bug from the other side. Set only on the success path so
// a failed load doesn't burn the one-shot; resets on page reload (module
// re-init).
let appWindowRestoreAttempted = false;

export function loadApps(): Promise<void> {
  if (appsLoadInFlight) return appsLoadInFlight;
  appsLoadInFlight = loadAppsInner().finally(() => { appsLoadInFlight = null; });
  return appsLoadInFlight;
}

async function loadAppsInner(): Promise<void> {
  setLoadingIfFresh(appsList);
  try {
    // Retry a transient rejection before flipping to `failed` — same reason as
    // `loadRepositoriesInner`: a failed apps list is never refetched on its own,
    // so one cancelled fetch strands every surface that reads it.
    const apps = await retryTransientRead(() => listAppsApi());
    appsList.value = { status: 'loaded', data: apps };

    // Restore previously open app window on reload, once per page load (see
    // `appWindowRestoreAttempted` above).
    // Set state directly instead of calling openApp() to avoid side effects
    // (switchMenuItem switches mobileView to 'content').
    if (!appWindowRestoreAttempted) {
      appWindowRestoreAttempted = true;
      const savedAppId = localStorage.getItem('app-window-open');
      if (savedAppId && !currentApp.value) {
        const saved = apps.find((s) => s.id === savedAppId);
        if (saved) {
          panelOverlay.value = { type: 'app-ui', app: saved };
        } else {
          // App was deleted while not open here, so drop the stale pointer and
          // the next reload won't keep probing for a missing id.
          localStorage.removeItem('app-window-open');
        }
      }
    }
  } catch (error) {
    appsList.value = toFailed(error);
  }
}

/** Open an app, optionally at an app fragment (docs/glossary.md): the place
 *  inside it a link named, delivered to the iframe as `location.hash`.
 *
 *  When the app was ALREADY open, two things happen that a cold open does not
 *  need. `replaceNavState` refreshes the kept entry, because `pushNavState`
 *  dedupes on app id and moving inside an open panel is a mutation in place. A
 *  reload then restores the newest target rather than the first one.
 *
 *  The hash also goes straight to the live frame, because the frame's own
 *  effect fires on a CHANGED src. An app that moved itself with
 *  `history.replaceState` leaves the src identical, so re-clicking the link the
 *  reader arrived on would otherwise deliver nothing. `setAppFrameHash` is
 *  idempotent, so the two writers cannot fight. */
export function openApp(app: App, fragment?: string): void {
  const wasOpen = currentApp.value?.id === app.id;
  panelOverlay.value = { type: 'app-ui', app, fragment };
  cancelPendingRefresh();
  if (appRefreshKey.value) appRefreshKey.value = 0;
  localStorage.setItem('app-window-open', app.id);
  revealContentPane();
  pushNavState();
  if (!wasOpen) return;
  replaceNavState();
  if (!fragment) return;
  const frame = getVisibleAppFrame();
  if (frame) setAppFrameHash(frame, fragment);
}

/** Open an app by ID — loads apps first if needed, then opens.
 *
 * Defense-in-depth on cache miss: if the cache is loaded but doesn't contain
 * the requested id, refetch once (a fresh `/apps` DISK scan) before erroring.
 * The backend reads disk directly while `appsList` is a cached projection
 * refreshed by per-channel hints (`AppCreated`/`AppUpdated`/`AppDeleted`,
 * `PluginInstalled`, …). This re-scan is the WRITER-AGNOSTIC safety net: it
 * finds an app no matter how it landed on disk — including apps created via
 * `run_bash`/`run_python` or any channel that emits no refresh hint, and a
 * brief Apply-vs-SSE race. So the navigate path never falsely reports a live
 * app as gone, even though the apps LIST panel itself may still lag for those
 * hint-less writers until reload. Only after this re-scan still misses do we
 * conclude the app is genuinely gone.
 *
 * The retry must NOT clobber `appsList` if it fails: a transient network blip
 * on the second fetch would otherwise turn the user's loaded list into the
 * `failed` Loadable, deleting cached data for one bad click. Snapshot
 * pre-fetch and restore on transient failure. */
export async function openAppById(
  appId: string,
  source?: string,
  fragment?: string,
): Promise<void> {
  // `source` describes where a navigate originated (e.g. a thread label, or
  // "an app") so a miss toast says where it came from instead of swallowing it.
  const from = source ? ` (requested by ${source})` : '';
  let apps = appsList.value;
  if (apps.status !== 'loaded') {
    await loadApps();
    apps = appsList.value;
  }
  if (apps.status !== 'loaded') {
    // loadApps stamped the failure on appsList (Loadable failed), but the user
    // who clicked the link is not on the apps tab — they'd see nothing.
    showToast(`Couldn't open app "${appId}"${from} — apps failed to load`, 'error');
    return;
  }
  let app = apps.data.find((s) => s.id === appId);
  if (!app) {
    const snapshot = appsList.value;
    await loadApps();
    const refreshed = appsList.value;
    if (refreshed.status === 'loaded') {
      app = refreshed.data.find((s) => s.id === appId);
    } else if (refreshed.status === 'failed' && snapshot.status === 'loaded') {
      // Retry failed transiently — restore the prior loaded data so one bad
      // click doesn't strip cached apps from every other surface in the UI.
      appsList.value = snapshot;
    }
  }
  if (app) {
    openApp(app, fragment);
  } else {
    // Disk re-scanned and the app genuinely isn't there. Name the id + source
    // so the user knows what was missing and where the navigate came from.
    console.warn(`[apps] navigate to missing app "${appId}"${from}`);
    showToast(`App "${appId}" no longer exists${from}`, 'error');
  }
}

export function openEditApp(appId: string): void {
  panelOverlay.value = { type: 'form', form: { type: 'app-edit', appId } };
  pushNavState();
  // Lands the edit form in the content pane: mobile swipe + desktop split
  // expand. Mirrors openApp (same view, same overlay surface) — without it a
  // click on a collapsed-split desktop silently looks like a no-op.
  revealContentPane();
}

export function closeAppForm(): void {
  closeInlineForm();
}

export async function saveAppMetadata(
  appId: string,
  name: string,
  description: string,
): Promise<boolean> {
  try {
    await updateAppApi(appId, { name, description });
    await loadApps();
    return true;
  } catch (error) {
    showToast('Failed to save app: ' + errorDetail(error), 'error');
    return false;
  }
}

export async function confirmDeleteApp(
  appId: string,
  appName: string
): Promise<void> {
  if (!(await showConfirm(`Delete app "${appName}"? This cannot be undone.`))) {
    return;
  }

  try {
    await deleteAppApi(appId);
    if (currentApp.value?.id === appId) {
      closeAppWindow();
    }
    void loadApps();
  } catch (error) {
    // A plugin-installed app can't be deleted directly — the engine 409s with
    // the owning plugin. Route the user to the plugin uninstall confirm panel
    // (the single removal authority) instead of dead-ending on an error toast.
    // Nothing is removed until the user confirms in that panel.
    if (error instanceof ApiError && error.httpCode === 409) {
      const body = error.body as { plugin_id?: string; plugin_name?: string } | undefined;
      if (body?.plugin_id) {
        try {
          const request = await stagePluginUninstall(body.plugin_id);
          openPluginUninstallRequest(request);
        } catch (e) {
          const label = body.plugin_name ?? body.plugin_id;
          showToast(`Couldn't open uninstall for plugin "${label}": ${errorDetail(e)}`, 'error');
        }
        return;
      }
    }
    showToast('Failed to delete app: ' + errorDetail(error), 'error');
  }
}

function closeAppWindow(): void {
  cancelPendingRefresh();
  if (appRefreshKey.value) appRefreshKey.value = 0;
  panelOverlay.value = null;
  localStorage.removeItem('app-window-open');
  pushNavState();
}

/** Debounce a flurry of AppUiRefreshRequested events (multi-file edit, agentic loop
 *  end + explicit refresh_app) into a single iframe reload. Module-scoped because
 *  there is at most one open app iframe at a time. */
let refreshDebounce: ReturnType<typeof setTimeout> | null = null;
const REFRESH_DEBOUNCE_MS = 150;

function cancelPendingRefresh(): void {
  if (refreshDebounce) {
    clearTimeout(refreshDebounce);
    refreshDebounce = null;
  }
}

export interface RefreshAppUiOptions {
  /** Keep wipPreviewThreadId set across the refresh. Set by the header
   *  refresh button, where the user explicitly wants to reload the WIP
   *  iframe (re-fetch worktree content) rather than fall back to live.
   *  The default — false — is correct for SSE-driven refreshes (Apply
   *  landed, worktree gone) and for direct file-edit saves through the
   *  app-source modal (live content changed, WIP overlay is stale). */
  preserveWip?: boolean;
}

export async function refreshAppUI(appId?: string, options: RefreshAppUiOptions = {}): Promise<void> {
  // AppUiRefreshRequested = the live app changed = a worktree merged in.
  // Drop any WIP preview targeting this app — the worktree it served is
  // gone (Apply removes it as part of ff-merge) and the WIP URL would
  // 404. Done BEFORE the `if (!app) return` early-exit because WIP can be
  // set for a thread whose target app isn't currently open in the
  // panel-overlay (button hidden but signal persists); the cleanup must
  // still fire so reopening the app lands on live, not a stale `?thread_id=`.
  // Predicate gates on the *requested* appId (closure arg) — using
  // currentApp.id wouldn't fire when no app is open.
  const refreshedAppId = appId ?? currentApp.value?.id;
  if (refreshedAppId && !options.preserveWip) {
    clearWipIfMatches((wipTid) => {
      const wipThread = threadMap.value.get(wipTid);
      const wipAppId = wipThread?.meta.codingAgentFolder?.split('/').filter(Boolean).pop();
      return wipAppId === refreshedAppId;
    });
  }

  // Pure refresh: reload the open iframe so it picks up on-disk changes.
  // Never opens the app — that's what app-link clicks and navigate_ui are for.
  // Otherwise an LLM that incidentally edits a file under apps/{id}/ would
  // pop the app pane open mid-conversation.
  const app = currentApp.value;
  if (!app) return;
  if (appId && app.id !== appId) return;

  cancelPendingRefresh();
  refreshDebounce = setTimeout(() => {
    refreshDebounce = null;
    appRefreshKey.value++;
  }, REFRESH_DEBOUNCE_MS);
}

/** Open the inline apps search bar. The bar focuses itself on mount. */
export function openAppSearch(): void {
  appSearchOpen.value = true;
}

/** Close the inline apps search bar and clear its query so the active tab
 *  shows its full list again. */
export function closeAppSearch(): void {
  appSearchOpen.value = false;
  appSearchQuery.value = '';
}

export function toggleAppSearch(): void {
  if (appSearchOpen.value) closeAppSearch();
  else openAppSearch();
}

export function createNewApp(): void {
  panelOverlay.value = { type: 'form', form: { type: 'new-app' } };
  pushNavState();
  // Lands the new-app form in the content pane: mobile swipe + desktop split
  // expand. Mirrors openApp (same view, same overlay surface) — without it a
  // click on a collapsed-split desktop silently looks like a no-op.
  revealContentPane();
}

export function submitNewApp(name: string, description: string): void {
  closeInlineForm();
  // Creating an app is always a fresh conversation, never a follow-up. Clear
  // the focused thread first so the queued message starts a NEW thread:
  // PromptInput's pendingChatMessage consumer calls sendMessage with no
  // explicit threadId, which otherwise falls back to focusedThreadId and would
  // append the create-app prompt to whatever thread the user had open.
  setFocusedThread(null);
  pendingChatMessage.value = `Create a new app called "${name}": ${description}`;
}

export function getAppFrameSrc(): string | null {
  const app = currentApp.value;
  if (!app) return null;
  // WIP preview (an app coding-agent thread's worktree) vs. the live workspace
  // copy. When no preview thread is set, serve live.
  const tid = wipPreviewThreadId.value;
  return appUrl(app.id, tid ?? undefined, currentAppFragment.value ?? undefined);
}

/** Open the app that's in the content pane as a top-level page of its own,
 *  outside the Lucidos shell (ADR 0044: the popout is a bare app tab).
 *
 *  The packaged desktop client ONLY. Everywhere else the control stays a real
 *  `<a target="_blank">`, which keeps cmd-click, middle-click and "copy link
 *  address" working and opens on the anchor's own user activation. That anchor
 *  is a dead click in the packaged client, for two reasons that stack: the app's
 *  href is root-relative, so the delegated `onGlobalClick` funnel (which claims
 *  `^https?://` only) never routes it; and WKWebView then asks wry's UI delegate
 *  to make the new window, which wry installs only when a builder called
 *  `.on_new_window()`. The app calls that for the in-app browser preview webview
 *  and nowhere else, so `main` (declared in `tauri.conf.json`) has no delegate
 *  and the navigation is dropped with no error anywhere.
 *
 *  Deliberately NOT `openUrl`: with the experimental in-app browser preference
 *  on, that mounts the url-preview panel, which is INSIDE the shell and would
 *  replace the very app the user asked to pop out of it. The OS opener is
 *  unconditional here, the same route `openLocalFile` takes. */
export function popOutApp(): void {
  const src = getAppFrameSrc();
  if (!src) {
    showToast("Couldn't open the app in a browser: no app is open", 'error');
    return;
  }
  // Absolute, resolved against this document, so the gateway origin, its port
  // and the workspace slug prefix all survive the hop out to the OS.
  const url = new URL(src, location.href).href;
  void openExternal(url).catch((err) =>
    showToast(`Couldn't open ${url}: ${errorDetail(err)}`, 'error'),
  );
}

export async function captureAppUI(appId: string, requestId: string): Promise<void> {
  // Safety-net: ensure we always respond before the backend's 10s timeout
  const deadline = new Promise<never>((_, reject) =>
    setTimeout(() => reject(new Error('Frontend capture deadline exceeded (8s)')), 8_000),
  );

  try {
    await Promise.race([captureAppUIInner(appId, requestId), deadline]);
  } catch (err) {
    // Telemetry carve-out (.claude/rules/frontend.md): the user-facing error
    // surface for capture is the LLM — postAppCapture below delivers it to
    // the backend, which inlines it as the tool result. The warn here is a
    // dev breadcrumb so the failure shows up in the page console too.
    console.warn('[App] UI capture failed:', err);
    // Best-effort response — may 404 if backend already timed out
    await postAppCapture(requestId, '', `Error: ${String(err)}`).catch(() => {});
  }
}

async function captureAppUIInner(appId: string, requestId: string): Promise<void> {
  const iframe = getVisibleAppFrame();

  if (!iframe) {
    await postAppCapture(requestId, '', 'Error: No app UI is currently open. Ask the user to open the app, or use navigate_ui (target=app-ui) first — refresh_app no longer opens.');
    return;
  }

  // If the captured app doesn't match the requested one, note the mismatch
  // so the LLM knows what it's actually seeing.
  const openAppId = currentApp.value?.id;
  const mismatchNote = (appId && openAppId && openAppId !== appId)
    ? `Note: Captured "${openAppId}" (currently open), not "${appId}" as requested.\n`
    : '';

  // Wait a tick for any pending renders
  await new Promise(r => setTimeout(r, 50));

  // Wait for lucidos._capture to become available (iframe may still be loading)
  type LucidosCaptureApi = { _capture: () => Promise<{ screenshot: string; dom: string }> };
  type LucidosWindow = Window & { lucidos?: Partial<LucidosCaptureApi> };
  let lucidosApi: LucidosCaptureApi | null = null;
  for (let i = 0; i < 15; i++) {
    const candidate = (iframe.contentWindow as LucidosWindow | null)?.lucidos;
    if (candidate?._capture) {
      lucidosApi = candidate as LucidosCaptureApi;
      break;
    }
    await new Promise(r => setTimeout(r, 100));
  }

  if (!lucidosApi) {
    await postAppCapture(requestId, '', 'Error: Capture function not available in iframe');
    return;
  }

  // Timeout the actual capture (html2canvas can hang on external resources)
  const captureTimeout = new Promise<never>((_, reject) =>
    setTimeout(() => reject(new Error('html2canvas capture timed out (5s)')), 5_000),
  );
  const result = await Promise.race([lucidosApi._capture(), captureTimeout]);
  await postAppCapture(requestId, result.screenshot, mismatchNote + result.dom);
}

/** Find the visible app-ui iframe — tolerates transient duplicates during a
 *  layout swap by preferring non-zero dimensions. */
export function getVisibleAppFrame(): HTMLIFrameElement | null {
  const frames = document.querySelectorAll('[data-role="app-ui-frame"]') as NodeListOf<HTMLIFrameElement>;
  let fallback: HTMLIFrameElement | null = null;
  for (const frame of frames) {
    if (!frame.contentWindow) continue;
    if (isElementVisible(frame)) return frame;
    fallback ??= frame;
  }
  return fallback;
}

/** The panel wrapping the visible app iframe: the element native fullscreen is
 *  requested on, and the one the host's overlay layer is portaled into while it
 *  is fullscreen.
 *
 *  Resolved from `getVisibleAppFrame` rather than by a second query, so the two
 *  cannot disagree about which app is on screen (that function carries the
 *  prefer-non-zero-dimensions tolerance for the transient duplicate frames a
 *  layout swap leaves behind).
 *
 *  Fullscreen is requested on the PANEL and not on the iframe because an iframe
 *  renders no DOM children: with the iframe fullscreen there is nowhere for the
 *  host to put a modal, and a fullscreen element is painted alone. */
export function getVisibleAppPanel(): HTMLElement | null {
  return getVisibleAppFrame()?.closest<HTMLElement>('[data-role="app-ui-panel"]') ?? null;
}

/** Exit CSS-based pseudo-fullscreen mode. */
export function exitPseudoFullscreen(): void {
  appPseudoFullscreen.value = false;
}

/** Leave whichever fullscreen mode an app panel is in, and report whether there
 *  was one to leave. The single definition of "come back to the normal layout",
 *  shared by the header's Fullscreen toggle and by navigation that has to make
 *  something OTHER than the app visible while keeping the app open (the
 *  `new-chat` navigate, which lands a prefilled compose in the thread pane).
 *
 *  Native first: the two modes are never active at once (the CSS fallback is
 *  taken only when the native request fails), and a natively fullscreen element
 *  is painted alone, so it is the one that actually hides the rest of the shell.
 *  Pseudo-fullscreen only stacks at `--z-app-fullscreen`, but it covers the
 *  viewport just the same. */
export function exitAppFullscreen(): boolean {
  if (nativeFullscreenElement() !== null) {
    const doc = document as unknown as Record<string, unknown>;
    if (typeof doc.exitFullscreen === 'function') {
      (doc.exitFullscreen as () => Promise<void>)().catch(() => { /* already leaving; failure is benign */ });
    } else if (typeof doc.webkitExitFullscreen === 'function') {
      (doc.webkitExitFullscreen as () => void)();
    }
    return true;
  }
  if (appPseudoFullscreen.value) {
    exitPseudoFullscreen();
    return true;
  }
  return false;
}
