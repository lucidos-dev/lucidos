// Thread presence — emit focused/unfocused signals to the backend so it can
// suppress redundant push notifications when the user is already viewing the
// thread that produced them.
//
// Triggers (driven by useStartup wiring):
// - `focusedThreadId` signal change → unfocus old, focus new
// - `visibilitychange` → unfocus when hidden, refocus when visible
// - `window blur/focus` → same as visibility (covers desktop window-not-foreground)
// - heartbeat every 30s while focused & visible → refresh focused_at
// - `beforeunload` → best-effort unfocus via sendBeacon

import { effect } from '@preact/signals';
import { API_BASE } from '../../api/client';
import { focusedThreadId } from '../store';
import { getDeviceId } from './devices';

const HEARTBEAT_INTERVAL_MS = 30_000;
const ENDPOINT = `${API_BASE}/api/thread-presence`;

/** Track what we last reported so we don't spam unchanged updates. */
let lastReported: { threadId: string; focused: boolean } | null = null;
let heartbeatTimer: ReturnType<typeof setInterval> | null = null;
let cleanupFns: Array<() => void> = [];

function postPresence(threadId: string, focused: boolean): void {
  // Best-effort POST — failures are logged but don't surface to the user.
  // Presence is a hint for notification suppression, not a critical data path.
  fetch(ENDPOINT, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      device_id: getDeviceId(),
      thread_id: threadId,
      focused,
    }),
    keepalive: true,
  }).catch((e) => {
    console.error('[Presence] Failed to post presence update:', e);
  });
}

/** True when the page is in a state where the user is actually looking at it. */
function isPageActive(): boolean {
  return document.visibilityState === 'visible' && document.hasFocus();
}

/** Send focused/unfocused for the current focused thread, given the page state. */
function syncPresence(): void {
  const threadId = focusedThreadId.value;
  if (!threadId) {
    // Nothing focused — if we previously reported focus, send an explicit unfocus.
    if (lastReported?.focused && lastReported.threadId) {
      postPresence(lastReported.threadId, false);
    }
    lastReported = null;
    stopHeartbeat();
    return;
  }

  const focused = isPageActive();
  // Skip if state unchanged.
  if (lastReported && lastReported.threadId === threadId && lastReported.focused === focused) {
    return;
  }

  // If we were previously focused on a different thread, send unfocus first
  // so the backend's projection doesn't end up with two rows for this device.
  if (lastReported?.focused && lastReported.threadId !== threadId) {
    postPresence(lastReported.threadId, false);
  }

  postPresence(threadId, focused);
  lastReported = { threadId, focused };

  if (focused) {
    startHeartbeat();
  } else {
    stopHeartbeat();
  }
}

function startHeartbeat(): void {
  if (heartbeatTimer !== null) return;
  heartbeatTimer = setInterval(() => {
    const threadId = focusedThreadId.value;
    if (!threadId || !isPageActive()) return;
    postPresence(threadId, true);
    lastReported = { threadId, focused: true };
  }, HEARTBEAT_INTERVAL_MS);
}

function stopHeartbeat(): void {
  if (heartbeatTimer !== null) {
    clearInterval(heartbeatTimer);
    heartbeatTimer = null;
  }
}

/** Wire all presence triggers. Call once at startup. Returns a teardown fn. */
export function startPresenceTracking(): () => void {
  // Tear down any previous wiring (defensive — useStartup runs once, but tests
  // and HMR can re-invoke).
  stopPresenceTracking();

  // Reactive: when focusedThreadId changes, sync.
  const disposeEffect = effect(() => {
    // Touch the signal so the effect subscribes to it.
    void focusedThreadId.value;
    syncPresence();
  });
  cleanupFns.push(disposeEffect);

  // Visibility/focus changes flip the page-active state.
  const onVisibility = () => syncPresence();
  const onFocus = () => syncPresence();
  const onBlur = () => syncPresence();
  document.addEventListener('visibilitychange', onVisibility);
  window.addEventListener('focus', onFocus);
  window.addEventListener('blur', onBlur);
  cleanupFns.push(() => document.removeEventListener('visibilitychange', onVisibility));
  cleanupFns.push(() => window.removeEventListener('focus', onFocus));
  cleanupFns.push(() => window.removeEventListener('blur', onBlur));

  // Best-effort: tell the backend we're leaving. sendBeacon survives unload.
  const onBeforeUnload = () => {
    const threadId = focusedThreadId.value;
    if (!threadId || !lastReported?.focused) return;
    try {
      const blob = new Blob([JSON.stringify({
        device_id: getDeviceId(),
        thread_id: threadId,
        focused: false,
      })], { type: 'application/json' });
      navigator.sendBeacon(ENDPOINT, blob);
    } catch {
      // Falls back to the regular onBlur which fires alongside unload anyway.
    }
  };
  window.addEventListener('beforeunload', onBeforeUnload);
  cleanupFns.push(() => window.removeEventListener('beforeunload', onBeforeUnload));

  return stopPresenceTracking;
}

export function stopPresenceTracking(): void {
  for (const fn of cleanupFns) fn();
  cleanupFns = [];
  stopHeartbeat();
  lastReported = null;
}
