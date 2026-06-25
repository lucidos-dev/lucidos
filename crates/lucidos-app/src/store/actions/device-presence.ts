// Device-level presence — emit visible/hidden whenever ANY Lucidos top-level
// tab on this device transitions visibility. Independent of `focusedThreadId`:
// even on the threads list (no thread focused) the device should report as
// visible so the backend can suppress cross-device push notifications.
//
// Triggers (driven by useStartup wiring):
// - `visibilitychange` → always POST current visibility (forceRefresh) — iOS
//   WebKit fires this on JS-suspend resume even when the page stayed visible,
//   so the lastReported dedupe would silently skip the post and let the
//   device_presence row age past 120s while the engine still thought we were
//   gone (see syncDevicePresence)
// - `window blur/focus` → also flips visibility (covers "window not foreground"),
//   deduped against lastReported because desktop fires them on every in-window
//   click and the bare visible↔hidden transitions reaching them are already
//   covered by visibilitychange
// - heartbeat every 30s while visible → refresh `visible_at` so the row stays fresh
// - `beforeunload` → best-effort hidden via sendBeacon
//
// Feeds the backend's PresenceCheck candidate index — see
// system-knowhow/notifications.md §3.

import { API } from '../../api/client';
import { getDeviceId } from './devices';
import { isPageActive } from '../../utils/pageActive';
import { startNativeWindowActiveTracking } from '../../utils/nativeWindow';

const HEARTBEAT_INTERVAL_MS = 30_000;
const ENDPOINT = `${API}/device-presence`;

let lastReported: boolean | null = null;
let heartbeatTimer: ReturnType<typeof setInterval> | null = null;
let cleanupFns: Array<() => void> = [];

function postDevicePresence(visible: boolean): void {
  fetch(ENDPOINT, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      device_id: getDeviceId(),
      visible,
    }),
    keepalive: true,
  }).catch((e) => {
    // Telemetry carve-out (.claude/rules/frontend.md): heartbeat runs on a
    // 30s interval without user intent. Failure just means a redundant push
    // notification next time — engine treats absent presence as "not active".
    console.warn('[DevicePresence] Failed to post:', e);
  });
}

function syncDevicePresence(opts: { forceRefresh?: boolean } = {}): void {
  const visible = isPageActive();
  // visible→visible is normally a no-op, but iOS WebKit fires
  // `visibilitychange` on JS-suspend resume even when the page never
  // dropped to hidden (system overlay, lockscreen, screen-off-then-on).
  // Without a fresh POST the row ages past PRESENCE_STALE_AFTER (120s)
  // while heartbeat ticks are stalled — next NotificationCreated then
  // sees zero candidates and fans out the OS push on top of an active
  // foreground PWA (notifications.md §2 row 4 misfire). Callers that own
  // a known iOS-suspend signal (visibilitychange) pass forceRefresh.
  if (!opts.forceRefresh && lastReported === visible) return;
  postDevicePresence(visible);
  lastReported = visible;
  if (visible) {
    startHeartbeat();
  } else {
    stopHeartbeat();
  }
}

function startHeartbeat(): void {
  if (heartbeatTimer !== null) return;
  heartbeatTimer = setInterval(() => {
    if (!isPageActive()) return;
    postDevicePresence(true);
    lastReported = true;
  }, HEARTBEAT_INTERVAL_MS);
}

function stopHeartbeat(): void {
  if (heartbeatTimer !== null) {
    clearInterval(heartbeatTimer);
    heartbeatTimer = null;
  }
}

export function startDevicePresenceTracking(): () => void {
  stopDevicePresenceTracking();

  // visibilitychange is the iOS-suspend recovery signal — see the
  // forceRefresh comment in syncDevicePresence. focus/blur stay deduped
  // because desktop fires them on every in-window click, and the bare
  // visible↔hidden transitions reaching them are already covered by the
  // visibilitychange branch.
  const onVisibilityChange = () => syncDevicePresence({ forceRefresh: true });
  const onFocusBlur = () => syncDevicePresence();
  document.addEventListener('visibilitychange', onVisibilityChange);
  window.addEventListener('focus', onFocusBlur);
  window.addEventListener('blur', onFocusBlur);
  cleanupFns.push(() => document.removeEventListener('visibilitychange', onVisibilityChange));
  cleanupFns.push(() => window.removeEventListener('focus', onFocusBlur));
  cleanupFns.push(() => window.removeEventListener('blur', onFocusBlur));

  const onBeforeUnload = () => {
    if (lastReported !== true) return;
    try {
      const blob = new Blob([JSON.stringify({
        device_id: getDeviceId(),
        visible: false,
      })], { type: 'application/json' });
      navigator.sendBeacon(ENDPOINT, blob);
    } catch {
      // Falls back to the regular onBlur which fires alongside unload anyway.
    }
  };
  window.addEventListener('beforeunload', onBeforeUnload);
  cleanupFns.push(() => window.removeEventListener('beforeunload', onBeforeUnload));

  // Tauri desktop only: the native window's active state (focus + tray) is
  // invisible to the webview's visibilitychange/focus events — macOS `orderOut:`
  // (trayed) fires nothing the WKWebView reports, and its hasFocus() is
  // unreliable. Bridge it so presence re-syncs the instant the native window goes
  // inactive/active, flipping the engine's candidate index immediately instead of
  // aging out over PRESENCE_STALE_AFTER (120s). nativeWindow.ts updates its cache
  // (read by isPageActive) BEFORE this onChange runs, so a plain (deduped)
  // syncDevicePresence coalesces with the webview's own focus/blur post for the
  // unfocused case yet still fires for the tray case the webview can't see (there
  // lastReported is still true, so the value genuinely changed). No-op off-Tauri.
  let nativeWindowCanceled = false;
  let stopNativeWindow: (() => void) | null = null;
  startNativeWindowActiveTracking(() => syncDevicePresence())
    .then((un) => {
      if (nativeWindowCanceled) un();
      else stopNativeWindow = un;
    })
    .catch(() => {
      // Best-effort wiring: even if this fails, the cache + isPageActive still
      // update via the listen; presence just lags to the next heartbeat. No user
      // intent here (telemetry carve-out, .claude/rules/frontend.md).
    });
  cleanupFns.push(() => {
    nativeWindowCanceled = true;
    stopNativeWindow?.();
  });

  syncDevicePresence();

  return stopDevicePresenceTracking;
}

export function stopDevicePresenceTracking(): void {
  for (const fn of cleanupFns) fn();
  cleanupFns = [];
  stopHeartbeat();
  lastReported = null;
}
