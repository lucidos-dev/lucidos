import {
  appsList,
  currentApp,
  panelOverlay,
  closeInlineForm,
  pendingChatMessage,
  showToast,
  showConfirm,
  appCommit,
  appPseudoFullscreen,
  appRefreshKey,
  wipPreviewThreadId,
  threadMap,
} from '../store';
import { clearWipIfMatches } from './wipPreview';
import { toFailed, setLoadingIfFresh } from '../types';
import type { App } from '../types';
import { revealContentPane } from './pane';
import {
  listAppsApi,
  deleteAppApi,
  updateAppApi,
  appUrl,
  postAppCapture,
} from '../../api/client';
import { pushNavState } from './navigation';
import { isElementVisible } from '../../components/chat/scrollState';
import { errorDetail } from '../../utils/errorDetail';

export async function loadApps(): Promise<void> {
  setLoadingIfFresh(appsList);
  try {
    const apps = await listAppsApi();
    appsList.value = { status: 'loaded', data: apps };

    // Restore previously open app window on reload.
    // Set state directly instead of calling openApp() to avoid side effects
    // (switchMenuItem switches mobileView to 'content').
    const savedAppId = localStorage.getItem('app-window-open');
    if (savedAppId && !currentApp.value) {
      const saved = apps.find((s) => s.id === savedAppId);
      if (saved) {
        panelOverlay.value = { type: 'app-ui', app: saved };
      } else {
        // App was deleted while not open here — drop the stale pointer so the
        // next reload doesn't keep probing for a missing id.
        localStorage.removeItem('app-window-open');
      }
    }
  } catch (error) {
    appsList.value = toFailed(error);
  }
}

export function openApp(app: App): void {
  panelOverlay.value = { type: 'app-ui', app };
  cancelPendingRefresh();
  if (appRefreshKey.value) appRefreshKey.value = 0;
  localStorage.setItem('app-window-open', app.id);
  revealContentPane();
  pushNavState();
}

/** Open an app by ID — loads apps first if needed, then opens.
 *
 * Defense-in-depth on cache miss: if the cache is loaded but doesn't contain
 * the requested id, refetch once before erroring. The backend search reads
 * disk directly while `appsList` is a cached projection refreshed by
 * `AppCreated`/`AppUpdated`/`AppDeleted` SSE events; a brief race between
 * Apply landing on disk and the SSE event arriving (or any future event-drop)
 * would otherwise produce "no app with id" for an app the search just found.
 *
 * The retry must NOT clobber `appsList` if it fails: a transient network blip
 * on the second fetch would otherwise turn the user's loaded list into the
 * `failed` Loadable, deleting cached data for one bad click. Snapshot
 * pre-fetch and restore on transient failure. */
export async function openAppById(appId: string): Promise<void> {
  let apps = appsList.value;
  if (apps.status !== 'loaded') {
    await loadApps();
    apps = appsList.value;
  }
  if (apps.status !== 'loaded') {
    // loadApps stamped the failure on appsList (Loadable failed), but the user
    // who clicked the link is not on the apps tab — they'd see nothing.
    showToast("Couldn't open app — apps failed to load", 'error');
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
    openApp(app);
  } else {
    showToast(`Couldn't open app — no app with id "${appId}"`, 'error');
  }
}

export function openEditApp(appId: string): void {
  panelOverlay.value = { type: 'form', form: { type: 'app-edit', appId } };
  pushNavState();
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
    showToast('Failed to delete app: ' + errorDetail(error), 'error');
  }
}

function closeAppWindow(): void {
  appCommit.value = null;
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

export function createNewApp(): void {
  panelOverlay.value = { type: 'form', form: { type: 'new-app' } };
  pushNavState();
}

export function submitNewApp(name: string, description: string): void {
  closeInlineForm();
  pendingChatMessage.value = `Create a new app called "${name}": ${description}`;
}

export function getAppFrameSrc(): string | null {
  const app = currentApp.value;
  if (!app) return null;
  // WIP preview wins over commit pinning — historical commit + live worktree
  // is nonsense (the worktree IS the current edit). Mutually exclusive at
  // the UI surface; if both ever co-occur, the WIP wins.
  const tid = wipPreviewThreadId.value;
  return appUrl(app.id, tid ? undefined : (appCommit.value ?? undefined), tid ?? undefined);
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

/** Exit CSS-based pseudo-fullscreen mode. */
export function exitPseudoFullscreen(): void {
  appPseudoFullscreen.value = false;
}
