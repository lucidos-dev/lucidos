import { isIOS } from './platform';

/** True when the user is actively viewing this tab/window.
 *
 *  Used by presence tracking (thread + device) to decide whether to report
 *  the device as "user is looking at it" — which the backend uses to
 *  suppress redundant push notifications.
 *
 *  Two-tier check:
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
  if (document.visibilityState !== 'visible') return false;
  if (isIOS()) return true;
  return document.hasFocus();
}
