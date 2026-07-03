// App-icon badge (Badging API). Sets the unread count on the installed PWA's
// icon. Shared by the workspace context (own-workspace count, see store/effects)
// and the gateway picker (aggregate total across running workspaces).
//
// Best-effort + feature-detected: browsers without the Badging API — and the
// Tauri WKWebView, which uses a native dock badge instead — simply no-op. A
// non-positive count clears the badge; a positive count sets the number.

type BadgingNavigator = Navigator & {
  setAppBadge?: (count?: number) => Promise<void>;
  clearAppBadge?: () => Promise<void>;
};

/** Mirror `count` onto the installed PWA's app-icon badge. No-op when the
 *  Badging API is unavailable. `count <= 0` clears the badge. */
export function applyAppBadge(count: number): void {
  const nav = navigator as BadgingNavigator;
  if (typeof nav.setAppBadge !== 'function') return;
  if (count > 0) {
    nav.setAppBadge(count).catch(() => {});
  } else {
    nav.clearAppBadge?.().catch(() => {});
  }
}
