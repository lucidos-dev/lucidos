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
