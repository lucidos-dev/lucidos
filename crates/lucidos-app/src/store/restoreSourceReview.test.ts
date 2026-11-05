import { expect, it } from 'vitest';
import { restoreSourceReview } from './restoreSourceReview';
import type { RestoreIntentRecord } from './restoreGenerationRecords';

function capture(pageId: string, owner?: string): RestoreIntentRecord {
  return { pageId, generation: 'restored', intent: { captureOwnerId: owner, pendingSelection: { model: 'pending' }, drafts: [
    { threadId: 'source', composerSource: { ownerId: 'original-owner', threadId: 'original-thread' },
      draft: { text: 'retained', image_hashes: [], mode: null }, selection: { model: 'chosen', sideQuestionMode: true } },
  ] } };
}

it('keeps unknown and live foreign composers available without automatic reconstruction', () => {
  const record = capture('foreign');
  expect(restoreSourceReview([record], null)[0]).toMatchObject({ ownership: 'unknown', automatic: false, kind: 'composer' });
  expect(restoreSourceReview([record], new Set(['original-owner']))[0]).toMatchObject({ ownership: 'live', automatic: false });
  expect(restoreSourceReview([record], new Set())[0]).toMatchObject({ ownership: 'orphan', automatic: true });
});

it('an authoritative empty owner capture supersedes foreign stubs without hiding their originals', () => {
  const foreign = capture('foreign');
  const owner = capture('owner', 'original-owner');
  owner.intent.drafts = [];
  const review = restoreSourceReview([foreign, owner], new Set());
  expect(review).toHaveLength(1);
  expect(review[0]).toMatchObject({ ownership: 'captured-owner', automatic: false, source: foreign.intent.drafts[0] });
  expect(foreign.intent.drafts).toHaveLength(1);
});

it('retains selection, pending selection and all destination generations without deduplicating archives', () => {
  const record = capture('owner', 'original-owner');
  record.materializations = [
    { generation: 'first', drafts: [{ sourceThreadId: 'source', threadId: 'destination' }], completed: ['destination'] },
    { generation: 'second', drafts: [{ sourceThreadId: 'source', threadId: 'next' }], completed: [] },
  ];
  const review = restoreSourceReview([record, capture('foreign')], null);
  expect(review).toHaveLength(2);
  expect(review[0]).toMatchObject({ ownership: 'authoritative', automatic: true,
    pendingSelection: { model: 'pending' }, source: { selection: { model: 'chosen', sideQuestionMode: true } },
    destinations: [{ generation: 'first', threadId: 'destination', completed: true },
      { generation: 'second', threadId: 'next', completed: false }] });
  expect(review[0].source).toBe(record.intent.drafts[0]);
  expect(review[0].pendingSelection).toBe(record.intent.pendingSelection);
});

it('marks retained queue revisions and unsent records as uncertain without dropping their full context', () => {
  const record = capture('retained');
  const queue = { ownerId: 'original-owner', threadId: 'original-thread', text: 'queued', localIds: ['file'],
    intent: { useCodingAgent: true, context: { paths: ['src/example.ts'] }, asSideQuestion: true } };
  record.intent.drafts = [
    { threadId: 'queue-first', draft: { text: 'queued', image_hashes: [], mode: null }, selection: {}, queuedUploadSend: queue },
    { threadId: 'queue-second', draft: { text: 'revised', image_hashes: [], mode: null }, selection: {}, queuedUploadSend: { ...queue, text: 'revised' } },
    { threadId: 'send', draft: { text: 'sent?', image_hashes: [], mode: null }, selection: {},
      unsentMessage: { eventId: 'event', ownerId: 'original-owner', threadId: 'original-thread', body: { message: 'sent?' },
        settlement: {}, sentAt: '2026-01-01T00:00:00Z', phase: 'sending' } as NonNullable<typeof record.intent.drafts[number]['unsentMessage']> },
  ];
  const review = restoreSourceReview([record], null);
  expect(review.map((r) => [r.kind, r.delivery])).toEqual([
    ['queued-send', 'unknown'], ['queued-send', 'unknown'], ['uncertain-send', 'unknown'],
  ]);
  expect(review[0].source.queuedUploadSend?.intent).toEqual(queue.intent);
  expect(review[1].source.queuedUploadSend?.text).toBe('revised');
  expect(review[2].source.unsentMessage).toBe(record.intent.drafts[2].unsentMessage);
});

it('links exact attachment content across source kinds without merging contexts or independent files', () => {
  const record = capture('archive');
  const upload = { localId: 'file', ownerId: 'owner', threadId: 'thread', name: 'image.png', mime: 'image/png',
    size: 1, createdAt: 1, bytes: new Uint8Array([1]).buffer };
  const composer = record.intent.drafts[0];
  record.intent.drafts = [
    { ...composer, threadId: 'composer', uploads: [upload] },
    { ...composer, threadId: 'queue', uploads: [{ ...upload, ownerId: 'adopted', sourceGeneration: 'later' }],
      queuedUploadSend: { ownerId: 'owner', threadId: 'thread', localIds: ['file'], text: 'queued', intent: { useCodingAgent: false, context: null } } },
    { ...composer, threadId: 'different-bytes', uploads: [{ ...upload, bytes: new Uint8Array([2]).buffer }] },
    { ...composer, threadId: 'independent-file', uploads: [{ ...upload, localId: 'other' }] },
  ];
  const review = restoreSourceReview([record], null);
  expect(review).toHaveLength(4);
  expect(review[0].related).toEqual([{ pageId: 'archive', generation: 'restored', sourceThreadId: 'queue' }]);
  expect(review[1].related).toEqual([{ pageId: 'archive', generation: 'restored', sourceThreadId: 'composer' }]);
  expect(review[2].related).toEqual([]);
  expect(review[3].related).toEqual([]);
  expect(record.intent.drafts).toHaveLength(4);
});

it('does not read archived bytes just to decide recovery admission', () => {
  const record = capture('owner', 'original-owner');
  const upload = { localId: 'file', ownerId: 'owner', threadId: 'thread', name: 'image.png', mime: 'image/png',
    size: 1, createdAt: 1, get bytes(): ArrayBuffer { throw new Error('bytes inspected'); } };
  record.intent.drafts = [
    { ...record.intent.drafts[0], threadId: 'first', uploads: [upload] },
    { ...record.intent.drafts[0], threadId: 'second', uploads: [upload] },
  ];
  const review = restoreSourceReview([record], null);
  expect(review.every((entry) => entry.automatic)).toBe(true);
  expect(() => review[0].related).toThrow('bytes inspected');
});
