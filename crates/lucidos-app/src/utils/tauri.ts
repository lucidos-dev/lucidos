declare global {
  interface Window {
    __TAURI_INTERNALS__?: {
      invoke: <T>(cmd: string, args?: Record<string, unknown>, options?: unknown) => Promise<T>;
      transformCallback: (callback: (response: unknown) => void, once?: boolean) => number;
    };
    /** Injected by the Tauri app on page load — CalVer version at build time. */
    __LUCIDOS_APP_VERSION__?: string;
  }
}

/** Invoke a Tauri command via IPC. Only call when isTauri() is true. */
export function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return window.__TAURI_INTERNALS__!.invoke<T>(cmd, args);
}

/**
 * Ref-counted panel webview visibility. Multiple overlays can request hide
 * independently; the webview only shows when all have released.
 */
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

/** Extract the text content and title from the panel webview. Only call when isTauri() is true. */
export async function getWebviewContent(): Promise<{ title: string; content: string }> {
  return invoke<{ title: string; content: string }>('webview_get_content');
}

/**
 * Tint the macOS window background to match the in-app header. Under
 * `titleBarStyle: "Overlay"` the webview paints the reclaimed title-bar band (the
 * `.titlebar-strip`); this colors the behind-the-webview fallback so that band
 * reads blue, not black, before the page paints. `color` is a CSS hex string (the
 * header-gradient top stop for the active theme). Sets the window layer only, so
 * the page background isn't tinted. Only call when isTauri() is true. */
export function setTitlebarColor(color: string): Promise<void> {
  return invoke('set_titlebar_color', { color });
}

/**
 * Start a native drag of the calling window. Used by `useWindowDragRegion` once
 * the pointer crosses the drag threshold over a non-interactive area of the
 * title-bar band. App command (always allowed) — replaces the ACL-blocked
 * `data-tauri-drag-region`. Best-effort; only call when isTauri() is true. */
export function startWindowDrag(): Promise<void> {
  return invoke('start_window_drag');
}

/**
 * Toggle the calling window between maximized (macOS zoom) and restored. Bound to
 * a double-click on the reclaimed title-bar strip only. Only call when isTauri()
 * is true. */
export function toggleWindowMaximize(): Promise<void> {
  return invoke('toggle_window_maximize');
}

/**
 * Show a native macOS notification banner via the app's own
 * `show_native_notification` command (notifications.rs). We drive Apple's modern
 * `UserNotifications` framework (`UNUserNotificationCenter`) in Rust — not
 * `tauri-plugin-notification` / `mac-notification-sys`, which sit on the
 * deprecated `NSUserNotification` API that no longer delivers on recent macOS —
 * and capture the tap via a delegate. `deepLink` is the SW-message shape
 * (`notification_id` /
 * `thread_id` / `event_id` / `tap`); on tap the command emits
 * `native-notification-tapped` with it, which the page routes through the same
 * dispatchDeepLink the web-push tap uses. Only call when isTauri() is true.
 */
export async function showNativeNotification(opts: {
  title: string;
  body: string;
  deepLink: Record<string, unknown>;
}): Promise<void> {
  // `link` (single word) matches the Rust command param name verbatim.
  await invoke('show_native_notification', {
    title: opts.title,
    body: opts.body,
    link: opts.deepLink,
  });
}

// --- App auto-update (packaged desktop app) ---

/** Check GitHub Releases for a newer signed packaged build. Returns the new
 *  version string when an update is available, else null. Drives the in-app
 *  update toast. Only call when isTauri() is true (no-op → null in dev). */
export function checkAppUpdate(): Promise<string | null> {
  return invoke<string | null>('check_app_update');
}

/** Install the available packaged update and restart the WHOLE stack — the
 *  launchd background service (gateway + engines + embedded Postgres) AND the GUI
 *  client — onto the new version. On success the client re-execs and this promise
 *  never resolves; it rejects with a string error otherwise (no update, download
 *  failure). Only call when isTauri() is true. */
export function installAppUpdateAndRestart(): Promise<void> {
  return invoke('install_app_update_and_restart');
}

// --- Mobile access (packaged desktop app; macOS) ---

/** Tailscale install/login state for the host Mac (mirror of the Rust
 *  `TailscaleInfo`). */
export interface TailscaleInfo {
  installed: boolean;
  running: boolean;
  hostname: string | null;
  url: string | null;
}

/** localhost / LAN / Tailscale connect URLs for the engine (mirror of the Rust
 *  `ConnectInfo`). */
export interface ConnectInfo {
  port: number;
  localhost_url: string;
  lan_ip: string | null;
  lan_url: string | null;
  tailscale: TailscaleInfo;
}

/** Open a URL in the system default browser (not the embedded webview). Only
 *  call when isTauri() is true. */
export function openExternal(url: string): Promise<void> {
  return invoke('open_url_external', { url });
}

/** Surface the engine's connect URLs (localhost / LAN / Tailscale). Only call
 *  when isTauri() is true. */
export function getConnectInfo(): Promise<ConnectInfo> {
  return invoke<ConnectInfo>('get_connect_info');
}

/** Bring the Mac onto a tailnet (`tailscale up`; interactive login). Rejects
 *  with a string error. Only call when isTauri() is true. */
export function tailscaleUp(authKey?: string): Promise<void> {
  return invoke('tailscale_up', { authKey: authKey ?? null });
}

/** Expose the engine over the tailnet (`tailscale serve`), returning the
 *  `…ts.net` URL. Rejects with a string error. Only call when isTauri() is true. */
export function tailscaleServe(): Promise<string> {
  return invoke<string>('tailscale_serve');
}

/** Listen for a Tauri event. Returns an unlisten function. Only call when isTauri() is true. */
export function listen<T>(event: string, handler: (e: { payload: T }) => void): Promise<() => void> {
  const internals = window.__TAURI_INTERNALS__!;
  const callbackId = internals.transformCallback((raw: unknown) => {
    handler(raw as { payload: T });
  });
  return internals.invoke<number>('plugin:event|listen', {
    event,
    target: { kind: 'Any' },
    handler: callbackId,
  }).then((id) => {
    return () => {
      internals.invoke('plugin:event|unlisten', { event, eventId: id }).catch(() => {});
    };
  });
}
