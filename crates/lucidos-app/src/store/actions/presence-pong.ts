/** Handler for the engine's `PresenceCheck` SSE broadcast. See
 *  system-knowhow/notifications.md §3 (pong protocol, freshness gate).
 *
 *  The check carries the toast. The page acts on it first (toast, auto-read,
 *  or the badge alone with toasts off) and then pongs the outcome. The engine
 *  suppresses the OS push only on an outcome saying the reader already has the
 *  notification in the app. So the push follows what the page DID, never what
 *  it promised: a page that hides after answering keeps its toast. */

import type { PresenceOutcome } from '@lucidos/event-stream';
import type { Tap } from '@lucidos/sdk';
import { getDeviceId } from './devices';
import { submitPong } from './event-stream';
import { presentInAppNotification } from './in-app-notification-toast';

export interface PresenceCheckPayload {
  notification_id: string;
  title: string;
  body: string;
  thread_id?: string | null;
  event_id?: string | null;
  app_id?: string | null;
  tap?: Tap | null;
  deadline_ms: number;
  /** Engine wall-clock at emit time. Drives the freshness gate. */
  sent_at_ms: number;
}

/** Act on a `PresenceCheck`, answer it, and return the outcome sent.
 *
 *  Null for a check past its deadline, which got no toast and no answer. The
 *  engine has decided by then, and has sent the push. A toast now would sit on
 *  top of it, typically an iOS PWA flushing its SSE queue after a push tap. */
export function handlePresenceCheck(payload: PresenceCheckPayload): PresenceOutcome | null {
  if (Date.now() - payload.sent_at_ms > payload.deadline_ms) return null;
  const outcome = presentInAppNotification({
    title: payload.title,
    body: payload.body,
    target: {
      notification: payload.notification_id,
      thread: payload.thread_id ?? null,
      event: payload.event_id ?? null,
      tap: payload.tap ?? null,
    },
  });
  // Through the transport, not straight to the engine. On a shared
  // connection the worker merges this with its other documents' answers. It
  // then POSTs one pong, which is the one the engine is waiting for.
  submitPong(payload.notification_id, { device_id: getDeviceId(), outcome });
  return outcome;
}
