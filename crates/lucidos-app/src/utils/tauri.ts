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
