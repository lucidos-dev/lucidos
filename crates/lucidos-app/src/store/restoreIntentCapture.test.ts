import { expect, it } from 'vitest';
import { captureRestoreIntent } from './restoreIntentCapture';
import type { PendingUploadRecord } from './pendingUploadRecords';
import type { UnsentMessageRecord } from './unsentMessageRecords';

const upload: PendingUploadRecord = {
  localId: 'image', threadId: 'draft', name: 'photo.png', mime: 'image/png',
  bytes: new Uint8Array([1, 2, 3]).buffer, size: 3, createdAt: 0, ownerId: 'old-page',
};
const unsent = {
  eventId: 'send', threadId: 'draft', ownerId: 'old-page',
  body: { mode: 'human', message: 'earlier send', image_hashes: ['hash'], model: 'chosen' },
} as UnsentMessageRecord;

it('captures production recovery sources after their writes drain, without adopting or sending them', async () => {
  const result = await captureRestoreIntent({
    readUploads: async () => ({ uploads: [upload], queuedUploadSendRecords: [{
      threadId: 'draft', ownerId: 'old-page', text: 'queued text', localIds: ['image'], intent: { useCodingAgent: false, context: null },
    }] }),
    readUnsent: async () => [unsent],
    readComposers: () => ({ drafts: new Map([['draft', { text: 'latest edit', image_hashes: [], mode: null }]]),
      selections: new Map([['draft', { model: 'current' }]]), pendingSelection: { codingAgent: 'codex' } }),
  });
  expect(result.drafts.map((d) => d.draft.text)).toEqual(['latest edit', 'earlier send', 'queued text', '']);
  expect(result.drafts[0].uploads).toEqual([upload]);
  expect(result.drafts[1].selection).toMatchObject({ model: 'chosen' });
  expect(result.unsentMessages).toEqual([unsent]);
  expect(result.pendingSelection).toEqual({ codingAgent: 'codex' });
  new Uint8Array(upload.bytes)[0] = 9;
  expect(new Uint8Array(result.drafts[0].uploads![0].bytes)[0]).toBe(1);
});

it('reads live composers after pending device writes and retains uploads without a loaded thread', async () => {
  let text = 'before';
  const result = await captureRestoreIntent({
    readUploads: async () => { text = 'while draining'; return { uploads: [upload], queuedUploadSendRecords: [] }; },
    readUnsent: async () => [],
    readComposers: () => ({ drafts: new Map([['other', { text, image_hashes: [], mode: null }]]),
      selections: new Map(), pendingSelection: {} }),
  });
  expect(result.drafts[0].draft.text).toBe('while draining');
  expect(result.drafts[1].uploads).toEqual([upload]);
});

it('fails closed when a recovery store cannot be read', async () => {
  await expect(captureRestoreIntent({
    readUploads: async () => { throw new Error('storage unavailable'); },
    readUnsent: async () => [],
    readComposers: () => { throw new Error('must not capture an incomplete source set'); },
  })).rejects.toThrow('storage unavailable');
});

it('preserves live upload bytes when the persistence cap left them out', async () => {
  const bytes = new Uint8Array([7, 8]).buffer;
  const result = await captureRestoreIntent({
    readUploads: async () => ({ uploads: [], queuedUploadSendRecords: [] }),
    readUnsent: async () => [],
    readComposers: () => ({ drafts: new Map(), selections: new Map(), pendingSelection: {},
      uploads: [{ ...upload, localId: 'not-persisted', bytes }], unsent: [unsent] }),
  });
  expect(result.drafts[0].uploads?.[0].localId).toBe('not-persisted');
  expect(result.unsentMessages).toEqual([unsent]);
  expect(result.drafts.some((draft) => draft.draft.text === 'earlier send')).toBe(true);
});

it('attaches send-only landed bytes to the unsent source without creating a blank composer', async () => {
  const landed = { ...upload, landedHash: 'hash' };
  const result = await captureRestoreIntent({
    readUploads: async () => ({ uploads: [landed], queuedUploadSendRecords: [] }),
    readUnsent: async () => [unsent],
    readComposers: () => ({ drafts: new Map(), selections: new Map(), pendingSelection: {} }),
  });
  expect(result.drafts).toHaveLength(1);
  expect(result.drafts[0].threadId).toBe('unsent:send');
  expect(result.drafts[0].uploads).toEqual([landed]);
});

it('keeps shared image bytes with both an explicit composer attachment and its earlier send', async () => {
  const landed = { ...upload, landedHash: 'hash' };
  const result = await captureRestoreIntent({
    readUploads: async () => ({ uploads: [landed], queuedUploadSendRecords: [] }),
    readUnsent: async () => [unsent],
    readComposers: () => ({ drafts: new Map([['draft', { text: 'next edit', image_hashes: ['hash'], mode: null }]]),
      selections: new Map(), pendingSelection: {} }),
  });
  expect(result.drafts).toHaveLength(2);
  expect(result.drafts.map((d) => d.uploads)).toEqual([[landed], [landed]]);
});

it('keeps foreign upload stubs separate from this page composer, with stable owner identity', async () => {
  const result = await captureRestoreIntent({
    ownerId: 'this-page',
    readUploads: async () => ({ uploads: [upload], queuedUploadSendRecords: [] }),
    readUnsent: async () => [],
    readComposers: () => ({ drafts: new Map([['draft', { text: 'my edit', image_hashes: [], mode: null }]]),
      selections: new Map(), pendingSelection: {} }),
  });
  expect(result.captureOwnerId).toBe('this-page');
  expect(result.drafts).toHaveLength(2);
  expect(result.drafts[0].uploads).toBeUndefined();
  expect(result.drafts[0].composerSource).toEqual({ ownerId: 'this-page', threadId: 'draft' });
  expect(result.drafts[1].composerSource).toEqual({ ownerId: 'old-page', threadId: 'draft' });
  expect(result.drafts[1].uploads).toEqual([upload]);
});

it('retains distinct queued intent and its review context across page captures', async () => {
  const queued = { threadId: 'draft', ownerId: 'old-page', text: 'queued text', localIds: ['image'],
    intent: { useCodingAgent: false, context: null } };
  const result = await captureRestoreIntent({
    ownerId: 'this-page',
    readUploads: async () => ({ uploads: [{ ...upload, landedHash: 'landed' }], queuedUploadSendRecords: [queued] }),
    readUnsent: async () => [],
    readComposers: () => ({ drafts: new Map(), selections: new Map(), pendingSelection: {},
      queued: [{ ...queued, ownerId: 'this-page', text: 'another queued text' }] }),
  });
  const sends = result.drafts.filter((d) => d.queuedUploadSend);
  expect(sends).toHaveLength(2);
  expect(sends.map((d) => d.draft.text)).toEqual(['queued text', 'another queued text']);
  expect(sends.map((d) => d.draft.image_hashes)).toEqual([['landed'], []]);
  expect(sends[0].queuedUploadSend).toEqual(queued);
  expect(sends[0].threadId).not.toBe(sends[1].threadId);
});

it('lets the live queued revision replace stale storage for the same owner and composer', async () => {
  const queued = { threadId: 'draft', ownerId: 'this-page', text: 'stored text', localIds: [],
    intent: { useCodingAgent: false, context: null } };
  const result = await captureRestoreIntent({
    ownerId: 'this-page',
    readUploads: async () => ({ uploads: [], queuedUploadSendRecords: [queued] }),
    readUnsent: async () => [],
    readComposers: () => ({ drafts: new Map(), selections: new Map(), pendingSelection: {},
      queued: [{ ...queued, text: 'live text' }] }),
  });
  expect(result.queuedUploadSends).toEqual([{ ...queued, text: 'live text' }]);
  expect(result.drafts.map((d) => d.draft.text)).toEqual(['live text']);
});

it.each(['owner', 'thread', 'generation'] as const)('keeps an upload separate from a queue with a different %s', async (difference) => {
  const queued = { threadId: upload.threadId, ownerId: upload.ownerId, sourceGeneration: null as string | null,
    text: 'queued', localIds: [upload.localId], intent: { useCodingAgent: false, context: null } };
  if (difference === 'owner') queued.ownerId = 'another-owner';
  if (difference === 'thread') queued.threadId = 'another-thread';
  if (difference === 'generation') queued.sourceGeneration = 'another-generation';
  const result = await captureRestoreIntent({ ownerId: 'capturing-page',
    readUploads: async () => ({ uploads: [upload], queuedUploadSendRecords: [queued] }),
    readUnsent: async () => [],
    readComposers: () => ({ drafts: new Map(), selections: new Map(), pendingSelection: {} }),
  });
  expect(result.drafts.find((d) => d.queuedUploadSend)?.uploads).toEqual([]);
  expect(result.drafts.find((d) => d.composerSource)?.uploads).toEqual([upload]);
});

it.each(['owner', 'thread', 'generation'] as const)('keeps landed bytes separate from an uncertain send with a different %s', async (difference) => {
  const send = { ...unsent };
  if (difference === 'owner') send.ownerId = 'another-owner';
  if (difference === 'thread') send.threadId = 'another-thread';
  if (difference === 'generation') send.sourceGeneration = 'another-generation';
  const landed = { ...upload, landedHash: 'hash' };
  const result = await captureRestoreIntent({ ownerId: 'capturing-page',
    readUploads: async () => ({ uploads: [landed], queuedUploadSendRecords: [] }),
    readUnsent: async () => [send],
    readComposers: () => ({ drafts: new Map(), selections: new Map(), pendingSelection: {} }),
  });
  expect(result.drafts.find((d) => d.unsentMessage)?.uploads).toEqual([]);
  expect(result.drafts.find((d) => d.composerSource)?.uploads).toEqual([landed]);
});
