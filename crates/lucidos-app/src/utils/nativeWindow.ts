// Authoritative native-window *active* state for the Tauri desktop client.
//
// The embedded WKWebView can't observe macOS `orderOut:` — a window dismissed to
// the menu-bar tray keeps `document.visibilityState === 'visible'` and
// `document.hasFocus() === true` (macOS posts no occlusion change for orderOut:)
// — and its `hasFocus()` is unreliable generally, so the page can't tell "in
// use" from "trayed / behind another app" on its own. The Rust side emits a
// `native-window-active` event (a bare boolean: focused AND on-screen) from the
// authoritative AppKit focus + hide/show state; we cache it and feed it into
// isPageActive() so a non-active desktop client gets the OS native banner instead
// of a suppressed, invisible in-app toast.
//
// Always `true` in the browser / PWA — no event ever arrives, so the default
// leaves web behavior unchanged. See system-knowhow/notifications.md §1, §4.

import { isTauri } from './platform';
import { listen } from './tauri';

let nativeWindowActive = true;

/** Whether the native (Tauri) window is currently active — focused and on-screen.
 *  Always `true` off-Tauri (no native bridge). Consulted by isPageActive(). */
export function isNativeWindowActive(): boolean {
  return nativeWindowActive;
}

/** Set the cached native-window active state. Called by the listen callback and
 *  by tests. Pure — only mutates the module cache. */
export function setNativeWindowActive(active: boolean): void {
  nativeWindowActive = active;
}

/** Subscribe to native-window active changes (Tauri only). Updates the cache
 *  BEFORE invoking `onChange`, so a handler that reads isNativeWindowActive() /
 *  isPageActive() sees the new value. Returns an unlisten; a no-op off-Tauri. */
export async function startNativeWindowActiveTracking(
  onChange?: (active: boolean) => void,
): Promise<() => void> {
  if (!isTauri()) return () => {};
  return listen<boolean>('native-window-active', (e) => {
    setNativeWindowActive(e.payload);
    onChange?.(e.payload);
  });
}
