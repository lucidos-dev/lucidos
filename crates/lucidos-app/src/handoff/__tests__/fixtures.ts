import type { PendingUploadRecord } from '../../store/pendingUploadRecords';
import type { UnsentMessageRecord } from '../../store/unsentMessageRecords';
import { OUTBOX_EXPORT_VERSION, OUTBOX_HANDOFF_MESSAGES, type OutboxExport, type OutboxWorkspace } from '../outboxHandoff';

/** A send the http page had in flight when the origin changed. */
export function unsentRecord(eventId: string): UnsentMessageRecord {
  return {
    eventId,
    threadId: 'thread-1',
    body: { message: 'hello' } as UnsentMessageRecord['body'],
    settlement: { kind: 'follow-up' } as UnsentMessageRecord['settlement'],
    sentAt: '2026-11-19T10:00:00Z',
    phase: 'sending',
    failedRetries: 0,
    ownerId: 'http-page',
  };
}

export function uploadRecord(localId: string): PendingUploadRecord {
  return {
    localId,
    threadId: 'thread-1',
    name: 'photo.png',
    mime: 'image/png',
    bytes: new ArrayBuffer(4),
    size: 4,
    createdAt: 1,
    ownerId: 'http-page',
  };
}

export function exportOf(workspaces: OutboxWorkspace[]): OutboxExport {
  return { kind: OUTBOX_HANDOFF_MESSAGES.export, version: OUTBOX_EXPORT_VERSION, workspaces };
}
