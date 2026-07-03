/** SDK handler for the engine's `PresenceCheck` SSE broadcast — see
 *  system-knowhow/notifications.md §3 (pong protocol, freshness gate).
 *
 *  PresenceCheck is a PURE pong trigger: it no longer renders the in-app
 *  toast. The toast used to fire here, synchronously on receipt — but that
 *  raced the engine's push decision (the pong round-trips back slower than
 *  the deadline on a slow link), so a foregrounded iOS PWA could get the
 *  toast AND the OS push for the same notification. The toast is now driven
 *  by `NotificationToastRequested` (§4), which the engine emits ONLY on the
 *  push-suppressed branch — so the two surfaces are mutually exclusive by the
 *  engine's single decision, not by a page-side timing race. */

import { API } from '../../api/client';
import { isPageActive } from '../../utils/pageActive';
import { isInViewport } from '../../utils/viewport';
import { focusedThreadId } from '../store';
import { getDeviceId } from './devices';

/** Wall-clock slack on top of `deadline_ms` before a PresenceCheck is
 *  considered stale (iOS PWA SSE-queue flush after a push tap). A late ping
 *  now only costs a discarded pong (the engine has already decided), but
 *  dropping it keeps the engine's pong accounting clean. Exported so tests
 *  assert against the same constant. */
export const STALE_GRACE_MS = 2000;

export interface PresenceCheckPayload {
  notification_id: string;
  event_id: string | null;
  deadline_ms: number;
  /** Engine wall-clock at emit time. Drives the freshness gate. */
  sent_at_ms: number;
}

export function handlePresenceCheck(payload: PresenceCheckPayload): void {
  if (Date.now() - payload.sent_at_ms > payload.deadline_ms + STALE_GRACE_MS) {
    return;
  }

  const body = {
    notification_id: payload.notification_id,
    device_id: getDeviceId(),
    is_active: isPageActive(),
    focused_thread_id: focusedThreadId.value,
    event_in_viewport: payload.event_id ? isInViewport(payload.event_id) : false,
  };

  // `keepalive: true` so the pong still flushes if the user navigates away
  // mid-send (e.g. notification fires as the tab is closing).
  fetch(`${API}/presence-pong`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    keepalive: true,
  }).catch((e) => {
    // Telemetry carve-out (.claude/rules/frontend.md): pong runs in response
    // to the engine's broadcast, not user intent. Engine's deadline-then-
    // default-to-push fallback covers a missed pong — the user still gets
    // the OS push instead of the in-app surface; no silent dead-end.
    console.warn('[PresencePong] Failed to POST:', e);
  });
}
