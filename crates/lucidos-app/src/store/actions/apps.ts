import { readWindowItem, removeWindowItem, writeWindowItem } from '../../utils/windowStorage';
import type { WidgetParams } from '../../utils/widgetParams';
import {
  appsList,
  currentApp,
  currentAppFragment,
  currentAppParams,
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
import { isTauri } from '../../utils/platform';
import { listen, setWindowFullscreen, WINDOW_FULLSCREEN_CHANGED_EVENT } from '../../utils/tauri';
import { APP_FRAME_ISOLATED } from '../../components/apps/appFrameSandbox';
import { warmAppIcons } from '../../components/shared/AppIcon';
import { askFrame } from './app-frame-channel';
import { CANVAS_APP_FRAME_SELECTOR } from '../../utils/appFrame';
import { reloadWidgetFrames } from '../widgets';

const CAPTURE_TIMEOUT_MS = 5_000;
import { clearWipIfMatches } from './wipPreview';
import { toFailed, setLoadingIfFresh } from '../types';
import { sharedSliceLoader, type SliceOf } from '../bootstrapSliceLoads';
import type { App } from '../types';
import type { CaptureFormat } from '../../generated/thread-event-wire';
import { revealContentPane } from './pane';
import {
  ApiError,
  listAppsApi,
  deleteAppApi,
  updateAppApi,
  stagePluginUninstall,
  appUrl,
  postAppCapture,
  retryTransient,
  getAppApi,
  recordAppOpenedApi,
} from '../../api/client';
import { pushNavState, replaceNavState } from './navigation';
import { setAppFrameHash } from '../../components/apps/iframeNav';
import { openPluginUninstallRequest } from './plugin-uninstall';
import { isElementVisible } from '../../utils/elementVisibility';
import { errorDetail } from '../../utils/errorDetail';
import { openExternal } from '../../utils/tauri';
import { BASE_PATH, REMOTE_GATEWAY } from '../../utils/basePath';
import { appLandingHash } from './appHash';

let appWindowRestoreAttempted = false;

export function loadApps(): Promise<void> {
  return appsLoader.load();
}

export async function refreshApps(): Promise<void> {
  const inFlight = appsLoader.inFlight();
  if (inFlight) await inFlight;
  return loadApps();
}

export const appsSlice: SliceOf<'apps'> = {
  begin: () => setLoadingIfFresh(appsList),
  apply: (apps) => {
    appsList.value = { status: 'loaded', data: apps };
    warmAppIcons(apps);

    if (!appWindowRestoreAttempted) {
      appWindowRestoreAttempted = true;
      const savedAppId = readWindowItem('app-window-open');
      if (savedAppId && !currentApp.value) {
        const saved = apps.find((s) => s.id === savedAppId);
        if (saved) {
          panelOverlay.value = { type: 'app-ui', app: saved };
        } else {
          void restoreOpenWidget(savedAppId);
        }
      }
    }
  },
  fail: (error) => { appsList.value = toFailed(error); },
};

const appsLoader = sharedSliceLoader('apps', appsSlice, () => retryTransient(() => listAppsApi()));

async function restoreOpenWidget(appId: string): Promise<void> {
  try {
    const app = await getAppApi(appId);
    if (currentApp.value) return;
    if (app.kind === 'widget') {
      panelOverlay.value = { type: 'app-ui', app };
      return;
    }
  } catch (error) {
    if (!(error instanceof ApiError && error.httpCode === 404)) return;
  }
  removeWindowItem('app-window-open');
}

export function openApp(app: App, fragment?: string, params?: WidgetParams): void {
  const wasOpen = currentApp.value?.id === app.id;
  panelOverlay.value = { type: 'app-ui', app, fragment, params };
  cancelPendingRefresh();
  if (appRefreshKey.value) appRefreshKey.value = 0;
  writeWindowItem('app-window-open', app.id);
  revealContentPane();
  pushNavState();
  if (!wasOpen) {
    recordAppOpened(app);
    return;
  }
  replaceNavState();
  if (!fragment) return;
  const frame = getVisibleAppFrame();
  if (frame) setAppFrameHash(frame, fragment);
}

function recordAppOpened(app: App): void {
  if (app.kind === 'widget') return;
  recordAppOpenedApi(app.id).catch((error) => {
    console.warn(`[apps] could not record opening ${app.id}: ${errorDetail(error)}`);
  });
}

export async function openAppById(
  appId: string,
  source?: string,
  fragment?: string,
): Promise<boolean> {
  const from = source ? ` (requested by ${source})` : '';
  let apps = appsList.value;
  if (apps.status !== 'loaded') {
    await loadApps();
    apps = appsList.value;
  }
  if (apps.status !== 'loaded') {
    showToast(`Couldn't open app "${appId}"${from}: apps failed to load`, 'error');
    return false;
  }
  let app = apps.data.find((s) => s.id === appId);
  if (!app) {
    const snapshot = appsList.value;
    await loadApps();
    const refreshed = appsList.value;
    if (refreshed.status === 'loaded') {
      app = refreshed.data.find((s) => s.id === appId);
    } else {
      if (refreshed.status === 'failed' && snapshot.status === 'loaded') appsList.value = snapshot;
      const reason = refreshed.status === 'failed' ? `: ${refreshed.error}` : '';
      showToast(`Couldn't open app "${appId}"${from}: the app list failed to reload${reason}`, 'error');
      return false;
    }
  }
  if (app) {
    openApp(app, fragment);
    return true;
  }
  try {
    openApp(await getAppApi(appId), fragment);
    return true;
  } catch (error) {
    if (!(error instanceof ApiError && error.httpCode === 404)) {
      showToast(`Couldn't open app "${appId}"${from}: ${errorDetail(error)}`, 'error');
      return false;
    }
    console.warn(`[apps] navigate to missing app "${appId}"${from}`);
    showToast(`App "${appId}" no longer exists${from}`, 'error');
    return false;
  }
}

export function openEditApp(appId: string): void {
  panelOverlay.value = { type: 'form', form: { type: 'app-edit', appId } };
  pushNavState();
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
  if (!(await showConfirm(`Delete app "${appName}"? This cannot be undone.`, 'Delete', { variant: 'danger' }))) {
    return;
  }

  try {
    await deleteAppApi(appId);
    if (currentApp.value?.id === appId) {
      closeAppWindow();
    }
    void loadApps();
  } catch (error) {
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
  removeWindowItem('app-window-open');
  pushNavState();
}

let refreshDebounce: ReturnType<typeof setTimeout> | null = null;
const REFRESH_DEBOUNCE_MS = 150;

function cancelPendingRefresh(): void {
  if (refreshDebounce) {
    clearTimeout(refreshDebounce);
    refreshDebounce = null;
  }
}

export interface RefreshAppUiOptions {
  preserveWip?: boolean;
}

export async function refreshAppUI(appId?: string, options: RefreshAppUiOptions = {}): Promise<void> {
  const refreshedAppId = appId ?? currentApp.value?.id;
  if (refreshedAppId) reloadWidgetFrames(refreshedAppId);
  if (refreshedAppId && !options.preserveWip) {
    clearWipIfMatches((wipTid) => {
      const wipThread = threadMap.value.get(wipTid);
      const wipAppId = wipThread?.meta.codingAgentFolder?.split('/').filter(Boolean).pop();
      return wipAppId === refreshedAppId;
    });
  }

  const app = currentApp.value;
  if (!app) return;
  if (appId && app.id !== appId) return;

  cancelPendingRefresh();
  refreshDebounce = setTimeout(() => {
    refreshDebounce = null;
    appRefreshKey.value++;
  }, REFRESH_DEBOUNCE_MS);
}

export function openAppSearch(): void {
  revealContentPane();
  if (appSearchOpen.value && typeof document !== 'undefined') document.querySelector<HTMLInputElement>('.apps-search-field input')?.focus({ preventScroll: true });
  appSearchOpen.value = true;
}

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
  revealContentPane();
}

export function submitNewApp(name: string, description: string): void {
  closeInlineForm();
  setFocusedThread(null);
  pendingChatMessage.value = `Create a new app called "${name}": ${description}`;
}

export function getAppFrameSrc(): string | null {
  const app = currentApp.value;
  if (!app) return null;
  const tid = wipPreviewThreadId.value;
  return appUrl(app.id, tid ?? undefined, currentAppFragment.value ?? undefined, currentAppParams.value);
}

export function appPopoutHref(): string | null {
  const app = currentApp.value;
  if (!app) return null;
  if (REMOTE_GATEWAY === null) return getAppFrameSrc();
  return `${BASE_PATH}/${appLandingHash({ appId: app.id, fragment: currentAppFragment.value ?? undefined })}`;
}

export function popOutApp(): void {
  const src = appPopoutHref();
  if (!src) {
    showToast("Couldn't open the app in a browser: no app is open", 'error');
    return;
  }
  const url = new URL(src, location.href).href;
  void openExternal(url).catch((err) =>
    showToast(`Couldn't open ${url}: ${errorDetail(err)}`, 'error'),
  );
}

export async function captureAppUI(appId: string, requestId: string, saveFormat?: CaptureFormat): Promise<void> {
  const deadline = new Promise<never>((_, reject) =>
    setTimeout(() => reject(new Error('Frontend capture deadline exceeded (8s)')), 8_000),
  );

  try {
    await Promise.race([captureAppUIInner(appId, requestId, saveFormat), deadline]);
  } catch (err) {
    // BEST-EFFORT: telemetry carve-out (.claude/rules/frontend.md). The user-facing error
    // surface for capture is the LLM — postAppCapture below delivers it to
    // the backend, which inlines it as the tool result. The warn here is a
    // dev breadcrumb so the failure shows up in the page console too.
    console.warn('[App] UI capture failed:', err);
    await postAppCapture(requestId, '', `Error: ${String(err)}`).catch(() => {});
  }
}

async function captureAppUIInner(appId: string, requestId: string, saveFormat?: CaptureFormat): Promise<void> {
  const iframe = getVisibleAppFrame();

  if (!iframe) {
    await postAppCapture(requestId, '', 'Error: No app UI is currently open. Open it with navigate_ui (target=app, app_id), then capture again. refresh_app does not open it.');
    return;
  }

  const openAppId = currentApp.value?.id;
  const mismatchNote = (appId && openAppId && openAppId !== appId)
    ? `Note: Captured "${openAppId}" (currently open), not "${appId}" as requested.\n`
    : '';

  await new Promise(r => setTimeout(r, 50));

  type CaptureResult = { screenshot: string; dom: string };
  let result: CaptureResult;
  if (APP_FRAME_ISOLATED) {
    try {
      result = await askFrame(iframe, 'capture', { format: saveFormat }, CAPTURE_TIMEOUT_MS) as CaptureResult;
    } catch (e) {
      await postAppCapture(requestId, '', `Error: ${errorDetail(e)}`);
      return;
    }
  } else {
    type LucidosCaptureApi = { _capture: (format?: CaptureFormat) => Promise<CaptureResult> };
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

    const captureTimeout = new Promise<never>((_, reject) =>
      setTimeout(() => reject(new Error('capture timed out (5s)')), CAPTURE_TIMEOUT_MS),
    );
    result = await Promise.race([lucidosApi._capture(saveFormat), captureTimeout]);
  }
  await postAppCapture(requestId, result.screenshot, mismatchNote + result.dom);
}

export function getVisibleAppFrame(): HTMLIFrameElement | null {
  const frames = document.querySelectorAll<HTMLIFrameElement>(CANVAS_APP_FRAME_SELECTOR);
  let fallback: HTMLIFrameElement | null = null;
  for (const frame of frames) {
    if (!frame.contentWindow) continue;
    if (isElementVisible(frame)) return frame;
    fallback ??= frame;
  }
  return fallback;
}

export function getVisibleAppPanel(): HTMLElement | null {
  return getVisibleAppFrame()?.closest<HTMLElement>('[data-role="app-ui-panel"]') ?? null;
}

export function exitPseudoFullscreen(): void {
  appPseudoFullscreen.value = false;
}

let windowCommands: Promise<void> = Promise.resolve();

let windowFullscreenOwned = false;

let windowExitEchoPending = false;

function syncWindowFullscreen(pseudo: boolean): void {
  windowCommands = windowCommands.then(async () => {
    if (pseudo) {
      const wasFullscreen = await setWindowFullscreen(true);
      windowFullscreenOwned = !wasFullscreen || windowExitEchoPending;
    } else if (windowFullscreenOwned) {
      windowFullscreenOwned = false;
      windowExitEchoPending = true;
      await setWindowFullscreen(false).catch((e) => {
        windowExitEchoPending = false;
        throw e;
      });
    }
  }).catch(() => {  });
}

export function installAppFullscreenWindowSync(): void {
  if (!isTauri()) return;
  appPseudoFullscreen.subscribe(syncWindowFullscreen);
  void listen<boolean>(WINDOW_FULLSCREEN_CHANGED_EVENT, (e) => {
    if (e.payload) return;
    if (windowExitEchoPending) {
      windowExitEchoPending = false;
      return;
    }
    windowFullscreenOwned = false;
    exitPseudoFullscreen();
  }).catch(() => {  });
}

export function exitAppFullscreen(): boolean {
  if (nativeFullscreenElement() !== null) {
    const doc = document as unknown as Record<string, unknown>;
    if (typeof doc.exitFullscreen === 'function') {
      (doc.exitFullscreen as () => Promise<void>)().catch(() => {  });
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

export function toggleAppFullscreen(): void {
  if (exitAppFullscreen()) return;

  if (isTauri()) {
    if (getVisibleAppFrame()) appPseudoFullscreen.value = true;
    return;
  }

  const frame = getVisibleAppFrame();
  if (!frame) return;

  const panel = getVisibleAppPanel();
  if (!panel) {
    appPseudoFullscreen.value = true;
    return;
  }

  const anyPanel = panel as unknown as Record<string, unknown>;
  const request = (typeof anyPanel.requestFullscreen === 'function' && anyPanel.requestFullscreen.bind(panel))
    || (typeof anyPanel.webkitRequestFullscreen === 'function' && anyPanel.webkitRequestFullscreen.bind(panel));

  if (request) {
    try {
      Promise.resolve(request() as Promise<void> | undefined)
        .then(() => frame.focus())
        .catch(() => {
          appPseudoFullscreen.value = true;
        });
    } catch {
      appPseudoFullscreen.value = true;
    }
  } else {
    appPseudoFullscreen.value = true;
  }
}
