
import type { PresenceOutcome } from '@lucidos/event-stream';
import type { Tap } from '@lucidos/sdk';
import { getDeviceId } from './devices';
import { submitPong } from './event-stream';
import { presentInAppNotification } from './in-app-notification-toast';
import { isInAudience } from './push-audience';

export interface PresenceCheckPayload {
  notification_id: string;
  title: string;
  body: string;
  thread_id?: string | null;
  event_id?: string | null;
  app_id?: string | null;
  tap?: Tap | null;
  deadline_ms: number;
  sent_at_ms: number;
  audience?: string[] | null;
}

export function handlePresenceCheck(payload: PresenceCheckPayload): PresenceOutcome | null {
  if (Date.now() - payload.sent_at_ms > payload.deadline_ms) return null;
  if (!isInAudience(payload.audience)) {
    submitPong(payload.notification_id, { device_id: getDeviceId(), outcome: 'not_shown' });
    return 'not_shown';
  }
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
  submitPong(payload.notification_id, { device_id: getDeviceId(), outcome });
  return outcome;
}
