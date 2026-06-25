import { isIOS } from './platform';
import { isNativeWindowActive } from './nativeWindow';

/** True when the user is actively viewing this tab/window.
 *
 *  Used by presence tracking (thread + device) to decide whether to report
 *  the device as "user is looking at it" — which the backend uses to
 *  suppress redundant push notifications.
 *
 *  Checks, in order:
 *  - Tauri desktop client: the native window must be active (focused AND
 *    on-screen). The embedded WKWebView can't observe macOS `orderOut:` (a
 *    window trayed to the menu bar keeps visibilityState=visible /
 *    hasFocus()=true) and its hasFocus() is unreliable generally, so the
 *    authoritative AppKit state is bridged in via utils/nativeWindow.ts. Always
 *    true in the browser / PWA (no native bridge), so this is a no-op there.
 *  - On desktop, both visibilityState=visible AND document.hasFocus() must
 *    be true. A tab being in the foreground stack isn't enough — the
 *    window has to actually have OS focus, since a background window is
 *    "visible" per spec but invisible in practice (covered by another app).
 *  - On iOS, only visibilityState matters. Safari and the standalone PWA
 *    can leave document.hasFocus() returning false even when the app is
 *    fully foregrounded (URL bar / system UI keeps the focus). Layering
 *    hasFocus() on top caused devices to silently report as hidden,
 *    suppressing presence and letting push notifications fire for events
 *    the user was actively reading on screen. */
export function isPageActive(): boolean {
  if (typeof document === 'undefined') return false;
  if (!isNativeWindowActive()) return false;
  if (document.visibilityState !== 'visible') return false;
  if (isIOS()) return true;
  return document.hasFocus();
}
