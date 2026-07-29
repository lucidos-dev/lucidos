// App-icon badge (Badging API). Sets the unread count on the installed PWA's
// icon. Shared by the workspace context (own-workspace count, see
// `syncWorkspaceAppBadge` below) and the gateway picker (aggregate total across
// running workspaces).
//
// Best-effort + feature-detected: browsers without the Badging API — and the
// Tauri WKWebView, which uses a native dock badge instead — simply no-op. A
// non-positive count clears the badge; a positive count sets the number.

import { unreadCount } from '../store';
import { IS_PICKER } from '../../utils/basePath';
import { isTauri } from '../../utils/platform';

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

/** Re-assert THIS workspace's app-icon badge from `unreadCount` — the same
 *  single source the bell badge and the Unread tab project from, so the two can
 *  never show different numbers.
 *
 *  Deliberately UNCONDITIONAL (it writes even when our own count didn't move),
 *  because the icon badge is an **externally written surface**: iOS sets it from
 *  the push payload's top-level `app_badge` in its parent process without ever
 *  running the page, and the service worker's `push` handler sets it on
 *  Chrome/Android — both while this page is backgrounded or closed. The page is
 *  therefore the only actor that knows the CURRENT truth, and it has to be able
 *  to overwrite a value it never saw being written.
 *
 *  This is why the `unreadCount` effect in `store/effects.ts` cannot be the only
 *  writer: a computed whose recomputed value is equal does not notify its
 *  subscribers, so a reload landing the SAME count re-runs nothing. Read the
 *  notification on another device, come back to a resident iOS PWA, and the
 *  count goes 0 → 0 while the icon still carries the 1 the push wrote — bell 0,
 *  icon 1, forever. Every path that (re)establishes the unread truth calls this
 *  (`loadUnreadNotifications`, the mark-read paths, resume).
 *
 *  Context-gated at CALL time: the gateway picker sets the cross-workspace
 *  aggregate itself (`WorkspacePicker.tsx`), and the Tauri desktop app drives a
 *  native dock badge / tray title from the gateway total. */
export function syncWorkspaceAppBadge(): void {
  if (IS_PICKER || isTauri()) return;
  applyAppBadge(unreadCount.value);
}
