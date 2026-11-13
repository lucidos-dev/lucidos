import { expect, it } from 'vitest';
import { preserveRestoreSources, queuedRestoreSourceId, restoreSourcePreserved } from './restoreAdmission';
import type { QueuedUploadSendRecord } from '../store/pendingUploadRecords';

it('suppresses only the captured queued record, allowing a later send on the same thread', () => {
  const original: QueuedUploadSendRecord = { threadId: 'thread', ownerId: 'before', text: 'old message',
    localIds: ['image'], intent: { useCodingAgent: false, context: { a: 1, b: 2 } } };
  preserveRestoreSources([], [], [queuedRestoreSourceId(original)]);
  const preserved = (record: QueuedUploadSendRecord) => restoreSourcePreserved('queued', queuedRestoreSourceId(record));
  expect(preserved(structuredClone(original))).toBe(true);
  expect(preserved({ ...original, sourceGeneration: 'restored' })).toBe(true);
  expect(preserved({ ...original, intent: { ...original.intent, context: { b: 2, a: 1 } } })).toBe(true);
  expect(preserved({ ...original, ownerId: 'after' })).toBe(false);
  expect(preserved({ ...original, text: 'new message' })).toBe(false);
  expect(preserved({ ...original, localIds: ['new-image'] })).toBe(false);
  expect(preserved({ ...original, intent: { ...original.intent, asSideQuestion: true } })).toBe(false);
});
