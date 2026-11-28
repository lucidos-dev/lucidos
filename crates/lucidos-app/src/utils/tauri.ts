import { recordIpcOutcome } from './ipcHealth';
import { landingName, type WorkspaceLanding } from './workspaceLanding';

declare global {
  interface Window {
    __TAURI_INTERNALS__?: {
      invoke: <T>(cmd: string, args?: Record<string, unknown>, options?: unknown) => Promise<T>;
      transformCallback: (callback: (response: unknown) => void, once?: boolean) => number;
      metadata?: {
        currentWindow?: { label?: string };
        currentWebview?: { label?: string };
      };
    };
    __LUCIDOS_APP_VERSION__?: string;
    __LUCIDOS_APP_RELEASE__?: string;
  }
}

export function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return window.__TAURI_INTERNALS__!.invoke<T>(cmd, args).then(
    (value) => {
      recordIpcOutcome(cmd);
      return value;
    },
    (error: unknown) => {
      recordIpcOutcome(cmd, error ?? new Error('rejected with no error'));
      throw error;
    },
  );
}

let hideCount = 0;

export function hidePanelWebview(): void {
  hideCount++;
  if (hideCount === 1) {
    invoke('hide_panel_webview').catch(() => {});
  }
}

export function showPanelWebview(): void {
  hideCount = Math.max(0, hideCount - 1);
  if (hideCount === 0) {
    invoke('show_panel_webview').catch(() => {});
  }
}

export function webviewGoBack(): void {
  invoke('webview_go_back').catch(() => {});
}

export function webviewGoForward(): void {
  invoke('webview_go_forward').catch(() => {});
}

export function webviewReload(url: string): void {
  invoke('navigate_panel_webview', { url }).catch(() => {});
}

export async function getWebviewContent(): Promise<{ title: string; content: string }> {
  return invoke<{ title: string; content: string }>('webview_get_content');
}

export function setTitlebarColor(color: string): Promise<void> {
  return invoke('set_titlebar_color', { color });
}

export function setTrafficLightOffset(barHeightPx: number): Promise<void> {
  return invoke('set_traffic_light_offset', { barHeightPx });
}

let readyToShowSignalled = false;
export function windowReadyToShow(): void {
  if (readyToShowSignalled) return;
  readyToShowSignalled = true;
  invoke('window_ready_to_show').catch((e) => console.warn('[window] ready-to-show failed', e));
}

export function startWindowDrag(): Promise<void> {
  return invoke('start_window_drag');
}

export function toggleWindowMaximize(): Promise<void> {
  return invoke('toggle_window_maximize');
}

export function setWindowFullscreen(fullscreen: boolean): Promise<boolean> {
  return invoke('set_window_fullscreen', { fullscreen });
}

export const WINDOW_FULLSCREEN_CHANGED_EVENT = 'window-fullscreen-changed';

export const VIEW_MENU_PANES = ['thread-drawer', 'thread-pane', 'content-pane', 'dashboard'] as const;

export type ViewMenuPane = (typeof VIEW_MENU_PANES)[number];

export const VIEW_MENU_ITEM_CLICKED_EVENT = 'view-menu-item-clicked';

export interface ViewMenuItemState {
  pane: ViewMenuPane;
  shown: boolean;
  enabled: boolean;
  accelerator: string | null;
}

export function setViewMenuState(items: readonly ViewMenuItemState[]): Promise<void> {
  return invoke('set_view_menu_state', { items });
}

export function setWindowTitle(title: string): Promise<void> {
  return invoke('set_window_title', { title });
}

export async function showNativeNotification(opts: {
  title: string;
  body: string;
  deepLink: Record<string, unknown>;
}): Promise<boolean | null> {
  return invoke<boolean | null>('show_native_notification', {
    title: opts.title,
    body: opts.body,
    link: opts.deepLink,
  });
}

export async function dismissNativeNotification(opts: {
  workspace: string | null;
  notificationId: string | null;
}): Promise<void> {
  await invoke('dismiss_native_notification', {
    workspace: opts.workspace,
    id: opts.notificationId,
  });
}

export function focusCallingWindow(): void {
  invoke('focus_calling_window').catch(() => {});
}

export function nudgeDockBadge(): void {
  invoke('nudge_dock_badge').catch(() => {});
}

export function getNativeWindowShown(): Promise<boolean> {
  return invoke<boolean>('get_native_window_shown');
}

export function getNativeWindowActive(): Promise<boolean> {
  return invoke<boolean>('get_native_window_active');
}

export function takePendingNativeTaps(
  workspace: string | null,
): Promise<Record<string, unknown>[]> {
  return invoke<Record<string, unknown>[]>('take_pending_native_taps', { workspace });
}

export function getOrCreateDeviceId(workspace: string, candidate: string): Promise<string> {
  return invoke<string>('get_or_create_device_id', { workspace, candidate });
}

export function previousDeviceId(workspace: string): Promise<string | null> {
  return invoke<string | null>('previous_device_id', { workspace });
}

export function rememberDeviceId(workspace: string, id: string): Promise<void> {
  return invoke<void>('remember_device_id', { workspace, id });
}


export interface AppUpdateOffer {
  version: string;
  notes: string | null;
}

function parseAppUpdateOffer(raw: unknown): AppUpdateOffer | null {
  if (raw === null || raw === undefined) return null;
  const offer = typeof raw === 'string' ? { version: raw } : (raw as Record<string, unknown>);
  const { version, notes } = offer;
  if (typeof version !== 'string' || version.trim() === '') {
    throw new Error(`check_app_update returned an unreadable offer: ${JSON.stringify(raw)}`);
  }
  return { version, notes: typeof notes === 'string' ? notes : null };
}

export async function checkAppUpdate(): Promise<AppUpdateOffer | null> {
  return parseAppUpdateOffer(await invoke<unknown>('check_app_update'));
}

export function installAppUpdateAndRestart(): Promise<void> {
  return invoke('install_app_update_and_restart');
}

export const APP_UPDATE_PROGRESS_EVENT = 'app-update-progress';

interface AppUpdateFrame {
  version: string | null;
}

export type AppUpdateProgress =
  | (AppUpdateFrame & { phase: 'checking' })
  | (AppUpdateFrame & { phase: 'downloading'; downloaded: number; total: number | null })
  | (AppUpdateFrame & { phase: 'verifying' })
  | (AppUpdateFrame & { phase: 'installing' })
  | (AppUpdateFrame & { phase: 'restarting-services' })
  | (AppUpdateFrame & { phase: 'relaunching' })
  | (AppUpdateFrame & { phase: 'cancelled' })
  | (AppUpdateFrame & { phase: 'failed'; message: string })
  | (AppUpdateFrame & { phase: 'bundle-swap-failed'; message: string });

export type AppUpdateRunning = Exclude<
  AppUpdateProgress,
  { phase: 'cancelled' | 'failed' | 'bundle-swap-failed' }
>;

export function cancelAppUpdate(): Promise<void> {
  return invoke('cancel_app_update');
}


export interface TailscaleInfo {
  installed: boolean;
  on_tailnet: boolean;
  reason: string | null;
  tailnet_ip: string | null;
  magic_dns_name: string | null;
  serve_url: string | null;
  cli_available: boolean;
}

export interface ConnectInfo {
  port: number;
  localhost_url: string;
  lan_ip: string | null;
  tailscale: TailscaleInfo;
}

export function openExternal(url: string): Promise<void> {
  return invoke('open_url_external', { url });
}

export interface SavedDownload {
  dir: string;
  path: string;
}

export function saveToDownloads(filename: string, contents: string): Promise<SavedDownload> {
  return invoke<SavedDownload>('save_to_downloads', { filename, contents });
}

export function showWorkspaceInNativeWindow(
  workspace: string,
  landing?: WorkspaceLanding,
): Promise<void> {
  return invoke('show_workspace_window', {
    workspace,
    landing: landing ? landingName(landing) : null,
  });
}

export function getConnectInfo(): Promise<ConnectInfo> {
  return invoke<ConnectInfo>('get_connect_info');
}

export function tailscaleUp(authKey?: string): Promise<void> {
  return invoke('tailscale_up', { authKey: authKey ?? null });
}

export function tailscaleServe(): Promise<string> {
  return invoke<string>('tailscale_serve');
}

export function cancelTailscaleServe(): Promise<void> {
  return invoke('cancel_tailscale_serve');
}

export const TAILSCALE_SERVE_PROGRESS_EVENT = 'tailscale-serve-progress';

export type TailscaleServeProgress =
  | { phase: 'starting' }
  | { phase: 'checking-tailnet' }
  | { phase: 'configuring' }
  | { phase: 'awaiting-tailnet-approval'; url: string }
  | { phase: 'waiting-for-https' }
  | { phase: 'done'; url: string }
  | { phase: 'failed'; message: string }
  | { phase: 'cancelled' };

export function currentWindowLabel(): string | null {
  const label = window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label;
  return typeof label === 'string' && label.length > 0 ? label : null;
}

export function listen<T>(event: string, handler: (e: { payload: T }) => void): Promise<() => void> {
  const internals = window.__TAURI_INTERNALS__!;
  const label = currentWindowLabel();
  const callbackId = internals.transformCallback((raw: unknown) => {
    handler(raw as { payload: T });
  });
  return internals.invoke<number>('plugin:event|listen', {
    event,
    target: label === null ? { kind: 'Any' } : { kind: 'AnyLabel', label },
    handler: callbackId,
  }).then((id) => {
    return () => {
      internals.invoke('plugin:event|unlisten', { event, eventId: id }).catch(() => {});
    };
  });
}
