import { expect, it } from 'vitest';
import { captureHistoricalSources } from './restoreHistoricalSources';
import type { PendingUploadRecord } from './pendingUploadRecords';

it('captures historical attachments without asserting an authoritative empty composer', async () => {
  const upload = { localId: 'old', ownerId: 'other', threadId: 'thread', bytes: new Uint8Array([3]).buffer } as PendingUploadRecord;
  const captured = await captureHistoricalSources('restored', {
    uploads: [upload, { ...upload, localId: 'current', sourceGeneration: 'restored' }], queuedUploadSendRecords: [],
  }, []);
  expect(captured.captureOwnerId).toBeUndefined();
  expect(captured.drafts).toHaveLength(1);
  expect(captured.drafts[0].uploads).toEqual([upload]);
  expect(captured.drafts[0].composerSource?.ownerId).toBe('other');
});

it('keeps current-generation bytes outside a historical queue', async () => {
  const upload = { localId: 'file', ownerId: 'other', threadId: 'thread', sourceGeneration: 'restored' } as PendingUploadRecord;
  const queue = { ownerId: 'other', threadId: 'thread', text: 'queued', localIds: ['file'], intent: { useCodingAgent: false, context: null } };
  const captured = await captureHistoricalSources('restored', { uploads: [upload], queuedUploadSendRecords: [queue] }, []);
  expect(captured.drafts).toHaveLength(1);
  expect(captured.drafts[0].queuedUploadSend).toEqual(queue);
  expect(captured.drafts[0].uploads).toEqual([]);
});


it('retains every queue revision and associates only its exact upload ownership and generation', async () => {
  const original = { localId: 'file', ownerId: 'owner', threadId: 'thread', sourceGeneration: 'old',
    name: 'photo', mime: 'image/png', size: 1, createdAt: 1, bytes: new Uint8Array([1]).buffer };
  const adopted = { ...original, ownerId: 'adopter' };
  const newer = { ...original, sourceGeneration: 'newer', bytes: new Uint8Array([2]).buffer };
  const queue = { ownerId: 'owner', threadId: 'thread', sourceGeneration: 'old',
    text: 'first', localIds: ['file'], intent: { useCodingAgent: false, context: null } };
  const captured = await captureHistoricalSources('restored', {
    uploads: [original, adopted, newer], queuedUploadSendRecords: [queue, { ...queue, text: 'second' }],
  }, []);
  expect(captured.drafts.filter((d) => d.queuedUploadSend).map((d) => d.draft.text)).toEqual(['first', 'second']);
  for (const draft of captured.drafts.filter((d) => d.queuedUploadSend)) expect(draft.uploads).toEqual([original]);
  const standalone = captured.drafts.filter((d) => !d.queuedUploadSend);
  expect(standalone).toHaveLength(2);
  expect(new Set(standalone.map((d) => d.threadId)).size).toBe(2);
  expect(standalone.flatMap((d) => d.uploads ?? [])).toEqual([adopted, newer]);
});
