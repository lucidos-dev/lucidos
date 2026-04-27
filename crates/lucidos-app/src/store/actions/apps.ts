import {
  appsList,
  currentApp,
  panelOverlay,
  closeInlineForm,
  pendingChatMessage,
  showToast,
  showConfirm,
  inputMode,
  appCommit,
  appPseudoFullscreen,
  appRefreshKey,
} from '../store';
import { toFailed } from '../types';
import type { App } from '../types';
import { navigateToPane } from './pane';
import { isMobile } from '../../utils/viewport';
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
  if (appsList.value.status !== 'loaded') {
    appsList.value = { status: 'loading' };
  }
  try {
    const apps = await listAppsApi();
    appsList.value = { status: 'loaded', data: apps };

    // Restore previously open app window on reload.
    // Set state directly instead of calling openApp() to avoid side effects
    // (openApp resets inputMode to 'do' and switchMenuItem switches mobileView to 'content').
    const savedAppId = localStorage.getItem('app-window-open');
    if (savedAppId && !currentApp.value) {
      const saved = apps.find((s) => s.id === savedAppId);
      if (saved) {
        panelOverlay.value = { type: 'app-ui', app: saved };
      }
    }
  } catch (error) {
    console.error('Error loading apps:', error);
    appsList.value = toFailed(error);
  }
}

export function openApp(app: App): void {
  panelOverlay.value = { type: 'app-ui', app };
  cancelPendingRefresh();
  if (appRefreshKey.value) appRefreshKey.value = 0;
  localStorage.setItem('app-window-open', app.id);
  inputMode.value = { type: 'do' };
  if (isMobile()) navigateToPane('content');
  pushNavState();
}

/** Open an app by ID — loads apps first if needed, then opens. */
export async function openAppById(appId: string): Promise<void> {
  let apps = appsList.value;
  if (apps.status !== 'loaded') {
    await loadApps();
    apps = appsList.value;
  }
  if (apps.status === 'loaded') {
    const app = apps.data.find((s) => s.id === appId);
    if (app) openApp(app);
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
    console.error('Failed to save app:', error);
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
    loadApps();
  } catch (error) {
    console.error('Error deleting app:', error);
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

/** Debounce a flurry of RefreshAppUI events (multi-file edit, agentic loop end +
 *  explicit refresh_app) into a single iframe reload. Module-scoped because there
 *  is at most one open app iframe at a time. */
let refreshDebounce: ReturnType<typeof setTimeout> | null = null;
const REFRESH_DEBOUNCE_MS = 150;

function cancelPendingRefresh(): void {
  if (refreshDebounce) {
    clearTimeout(refreshDebounce);
    refreshDebounce = null;
  }
}

export async function refreshAppUI(appId?: string): Promise<void> {
  let app = currentApp.value;

  // If the target app isn't open, open it first so the iframe exists.
  if (!app && appId) {
    await openAppById(appId);
    app = currentApp.value;
  }

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
  return appUrl(app.id, appCommit.value ?? undefined);
}

export async function captureAppUI(appId: string, requestId: string): Promise<void> {
  // Safety-net: ensure we always respond before the backend's 10s timeout
  const deadline = new Promise<never>((_, reject) =>
    setTimeout(() => reject(new Error('Frontend capture deadline exceeded (8s)')), 8_000),
  );

  try {
    await Promise.race([captureAppUIInner(appId, requestId), deadline]);
  } catch (err) {
    console.error('App UI capture failed:', err);
    // Best-effort response — may 404 if backend already timed out
    await postAppCapture(requestId, '', `Error: ${String(err)}`).catch(() => {});
  }
}

async function captureAppUIInner(appId: string, requestId: string): Promise<void> {
  // Find the visible app-ui iframe. Both desktop (SplitLayout) and mobile
  // (MobileSwipeContainer) render ContentPane simultaneously, so there may be
  // two iframes. Prefer the one with non-zero dimensions (the visible layout).
  const iframe = getVisibleAppFrame();

  if (!iframe) {
    await postAppCapture(requestId, '', 'Error: No app UI is currently open. Use refresh_app to open it first.');
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

/** Find the visible app-ui iframe, preferring the one with non-zero dimensions
 *  to handle dual-rendering (desktop + mobile layouts render simultaneously). */
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

export { openCredentialRequest } from './credentials';
