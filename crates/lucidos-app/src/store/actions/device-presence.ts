
import { API, mutatingFetch } from '../../api/client';
import { getDeviceId } from './devices';
import { isPageActive } from '../../utils/pageActive';
import { startNativeWindowActiveTracking } from '../../utils/nativeWindow';

const HEARTBEAT_INTERVAL_MS = 30_000;
const ENDPOINT = `${API}/device-presence`;

let lastReported: boolean | null = null;
let heartbeatTimer: ReturnType<typeof setInterval> | null = null;
let cleanupFns: Array<() => void> = [];

function postDevicePresence(visible: boolean): Promise<void> {
  return mutatingFetch(ENDPOINT, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      device_id: getDeviceId(),
      visible,
    }),
    keepalive: true,
  }).then(() => {}, (e) => {
    // BEST-EFFORT: telemetry carve-out (.claude/rules/frontend.md). The heartbeat
    // runs on a 30s interval without user intent.
    // Failure just means a redundant push
    // notification next time — engine treats absent presence as "not active".
    console.warn('[DevicePresence] Failed to post:', e);
  });
}

function syncDevicePresence(opts: { forceRefresh?: boolean } = {}): void {
  const visible = isPageActive();
  if (!opts.forceRefresh && lastReported === visible) return;
  void postDevicePresence(visible);
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
    void postDevicePresence(true);
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
    void postDevicePresence(false);
  };
  window.addEventListener('beforeunload', onBeforeUnload);
  cleanupFns.push(() => window.removeEventListener('beforeunload', onBeforeUnload));

  let nativeWindowCanceled = false;
  let stopNativeWindow: (() => void) | null = null;
  startNativeWindowActiveTracking(() => syncDevicePresence())
    .then((un) => {
      if (nativeWindowCanceled) un();
      else stopNativeWindow = un;
    })
    .catch(() => {
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
