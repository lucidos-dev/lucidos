import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { PreservedComposeIntent } from './restoreGenerationRecords';

// Model IndexedDB's serialized transactions and structured cloning. These
// tests check the journal's concurrency and failure contract without opening
// any live device store or importing the application signals.
const device = vi.hoisted(() => ({
  databases: new Map<string, Map<string, unknown>>(),
  tails: new Map<string, Promise<unknown>>(),
  failNextCommit: false,
}));

vi.mock('./deviceDatabase', async (original) => {
  const actual = await original<typeof import('./deviceDatabase')>();
  return {
    ...actual,
    openDeviceDatabase(name: string) {
      return {
        transact(storeName: string, mode: IDBTransactionMode, body: (store: IDBObjectStore) => unknown) {
          const key = `${name}/${storeName}`;
          const run = (device.tails.get(key) ?? Promise.resolve()).then(async () => {
            const saved = device.databases.get(key) ?? new Map();
            const staged = structuredClone(saved);
            const events: Array<() => void> = [];
            const store = {
              get(id: string) {
                const req = { result: structuredClone(staged.get(id)), onsuccess: null as null | (() => void) };
                events.push(() => req.onsuccess?.());
                return req;
              },
              put(value: unknown, id: string) { staged.set(id, structuredClone(value)); },
            } as unknown as IDBObjectStore;
            const req = body(store) as IDBRequest | undefined;
            for (const event of events) event();
            if (mode === 'readwrite') {
              if (device.failNextCommit) {
                device.failNextCommit = false;
                throw new Error('transaction aborted');
              }
              device.databases.set(key, staged);
            }
            return req?.result;
          });
          device.tails.set(key, run.catch(() => {}));
          return run;
        },
      };
    },
  };
});

import { createRestoreGenerationStore } from './restoreGenerationRecords';

function intent(text: string): PreservedComposeIntent {
  return {
    drafts: [{
      threadId: 'draft-before-restore',
      draft: { text, image_hashes: ['image-before-restore'], mode: 'lucidos' },
      selection: { scope: { kind: 'lucidos' }, model: 'selected-model', reasoningEffort: 'high' },
    }],
    pendingSelection: { codingAgent: 'codex' },
  };
}

beforeEach(() => {
  device.databases.clear();
  device.tails.clear();
  device.failNextCommit = false;
});

describe('device restore generation journal', () => {
  it('distinguishes a first observation from an observed unrestored workspace', async () => {
    const store = createRestoreGenerationStore('alpha', 'page-a');
    expect(await store.readGeneration()).toBeUndefined();
    await store.observe(null);
    expect(await store.readGeneration()).toBeNull();
    expect(await store.readRecoveries()).toEqual([]);
  });

  it('preserves draft text, images, mode, per-draft and pending selections across page creation', async () => {
    const store = createRestoreGenerationStore('alpha', 'page-a');
    await store.preserve('restore-a', intent('typed before restore'));
    const nextPage = createRestoreGenerationStore('alpha', 'page-b');
    expect(await nextPage.readGeneration()).toBe('restore-a');
    expect(await nextPage.readRecoveries()).toEqual([
      { pageId: 'page-a', generation: 'restore-a', intent: intent('typed before restore') },
    ]);
  });

  it('does not advance the generation or lose earlier intent on an aborted commit', async () => {
    const store = createRestoreGenerationStore('alpha', 'page-a');
    await store.preserve('restore-a', intent('earlier'));
    device.failNextCommit = true;
    await expect(store.preserve('restore-b', intent('later'))).rejects.toThrow('transaction aborted');
    expect(await store.readGeneration()).toBe('restore-a');
    expect((await store.readRecoveries()).map((r) => r.intent.drafts[0].draft.text)).toEqual(['earlier']);
    await store.preserve('restore-b', intent('later'));
    expect(await store.readGeneration()).toBe('restore-b');
    expect(await store.readRecoveries()).toHaveLength(2);
  });

  it('retains captures from two tabs racing to observe a restore', async () => {
    const a = createRestoreGenerationStore('alpha', 'page-a');
    const b = createRestoreGenerationStore('alpha', 'page-b');
    await Promise.all([a.preserve('restore-a', intent('tab a')), b.preserve('restore-a', intent('tab b'))]);
    expect((await a.readRecoveries()).map((r) => r.intent.drafts[0].draft.text)).toEqual(['tab a', 'tab b']);
  });

  it('keeps unrecovered captures when a fresh page simply observes the same generation', async () => {
    const a = createRestoreGenerationStore('alpha', 'page-a');
    await a.preserve('restore-a', intent('unreviewed'));
    await createRestoreGenerationStore('alpha', 'page-b').observe('restore-a');
    expect(await a.readRecoveries()).toHaveLength(1);
  });

  it('makes a lost commit response retry idempotent for this page and generation', async () => {
    const a = createRestoreGenerationStore('alpha', 'page-a');
    await a.preserve('restore-a', intent('original capture'));
    await a.preserve('restore-a', intent('retry'));
    expect(await a.readRecoveries()).toEqual([
      { pageId: 'page-a', generation: 'restore-a', intent: intent('original capture') },
    ]);
  });

  it('never reads or changes another workspace on the shared origin', async () => {
    const a = createRestoreGenerationStore('alpha', 'page-a');
    const b = createRestoreGenerationStore('beta', 'page-a');
    await a.preserve('restore-a', intent('workspace alpha'));
    expect(await b.readGeneration()).toBeUndefined();
    expect(await b.readRecoveries()).toEqual([]);
  });

  it('does not retain mutable references to captured or returned intent', async () => {
    const a = createRestoreGenerationStore('alpha', 'page-a');
    const capture = intent('original');
    await a.preserve('restore-a', capture);
    capture.drafts[0].draft.text = 'changed';
    const records = await a.readRecoveries();
    records[0].intent.drafts[0].selection.model = 'changed-model';
    expect((await a.readRecoveries())[0].intent).toEqual(intent('original'));
  });
});

describe('durable restored draft destinations', () => {
  it('allocates one destination per captured draft and shares it across competing tabs', async () => {
    const a = createRestoreGenerationStore('alpha', 'page-a');
    const b = createRestoreGenerationStore('alpha', 'page-b');
    await a.preserve('restore-a', intent('review me'));
    const source = (await a.readRecoveries())[0];
    const [first, second] = await Promise.all([
      a.prepareRecovery(source, 'restore-a', () => 'fresh-a'),
      b.prepareRecovery(source, 'restore-a', () => 'fresh-b'),
    ]);
    expect(first).toEqual(second);
    expect(first?.drafts[0]).toEqual({ sourceThreadId: 'draft-before-restore', threadId: 'fresh-a' });
    expect(first?.completed).toEqual([]);
  });

  it('keeps the same destination after a reload and failed acknowledgement', async () => {
    const a = createRestoreGenerationStore('alpha', 'page-a');
    await a.preserve('restore-a', intent('review me'));
    const source = (await a.readRecoveries())[0];
    const prepared = await a.prepareRecovery(source, 'restore-a', () => 'fresh-a');
    device.failNextCommit = true;
    await expect(a.completeRecoveryDraft(source, 'restore-a', 'fresh-a')).rejects.toThrow();
    const b = createRestoreGenerationStore('alpha', 'page-b');
    expect(await b.prepareRecovery(source, 'restore-a', () => 'unused')).toEqual(prepared);
    await b.completeRecoveryDraft(source, 'restore-a', 'fresh-a');
    expect((await a.prepareRecovery(source, 'restore-a', () => 'unused'))?.completed).toEqual(['fresh-a']);
    expect((await a.readRecoveries())[0].intent).toEqual(intent('review me'));
  });

  it('reconstructs into new destinations after another restore without adopting archived completion', async () => {
    const a = createRestoreGenerationStore('alpha', 'page-a');
    await a.preserve('restore-a', intent('review me'));
    const source = (await a.readRecoveries())[0];
    await a.prepareRecovery(source, 'restore-a', () => 'fresh-a');
    await a.completeRecoveryDraft(source, 'restore-a', 'fresh-a');
    await a.observe('restore-b');
    expect(await a.prepareRecovery(source, 'restore-a', () => 'stale')).toBeUndefined();
    const next = await a.prepareRecovery(source, 'restore-b', () => 'fresh-b');
    expect(next?.drafts[0].threadId).toBe('fresh-b');
    expect(next?.completed).toEqual([]);
    await expect(a.completeRecoveryDraft(source, 'restore-a', 'fresh-a')).rejects.toThrow();
  });

  it('rejects unknown destinations and source records without losing preserved intent', async () => {
    const a = createRestoreGenerationStore('alpha', 'page-a');
    await a.preserve('restore-a', intent('review me'));
    const source = (await a.readRecoveries())[0];
    await a.prepareRecovery(source, 'restore-a', () => 'fresh-a');
    await expect(a.completeRecoveryDraft(source, 'restore-a', 'unknown')).rejects.toThrow();
    expect(await a.prepareRecovery({ ...source, pageId: 'missing' }, 'restore-a', () => 'unused')).toBeUndefined();
    expect((await a.readRecoveries())[0].intent).toEqual(intent('review me'));
  });
});

describe('cross-tab generation compare and commit', () => {
  it('cannot replace another tab\'s newer observation with a late old hello', async () => {
    const a = createRestoreGenerationStore('alpha', 'page-a');
    await a.observe(null);
    await a.observe('restore-a');
    await expect(a.observe(null, { generation: null })).rejects.toThrow('changed in another tab');
    expect(await a.readGeneration()).toBe('restore-a');
  });

  it('keeps both captures when tabs observed the same old generation', async () => {
    const a = createRestoreGenerationStore('alpha', 'page-a');
    const b = createRestoreGenerationStore('alpha', 'page-b');
    await a.observe(null);
    await a.preserve('restore-a', intent('tab a'), { generation: null });
    await b.preserve('restore-a', intent('tab b'), { generation: null });
    expect(await a.readRecoveries()).toHaveLength(2);
    await expect(b.preserve('restore-old', intent('stale'), { generation: null })).rejects.toThrow('changed in another tab');
    expect(await a.readRecoveries()).toHaveLength(2);
  });
});

// Exercise the real barrier, journal and reconstruction together. The server
// stand-in creates once, modelling a lost response after durable creation.
import { RestoreGenerationBarrier } from './restoreGeneration';
import { recoverRestoreDrafts } from './restoreDraftRecovery';

it('preserves a changed-generation page then resumes reconstruction after a lost response without overwriting edits', async () => {
  const oldStore = createRestoreGenerationStore('alpha', 'old-page');
  await oldStore.observe(null);
  const reload = vi.fn();
  const barrier = new RestoreGenerationBarrier(oldStore, { capture: async () => intent('before restore'), reload });
  const epoch = barrier.beginConnection();
  expect(await barrier.acceptHello(epoch, 'restore-a')).toBe('reload');
  expect(barrier.isReady(epoch)).toBe(false);
  expect(reload).toHaveBeenCalledOnce();

  const newStore = createRestoreGenerationStore('alpha', 'new-page');
  const freshBarrier = new RestoreGenerationBarrier(newStore, { capture: async () => intent('unused'), reload });
  const connection = freshBarrier.beginConnection();
  expect(await freshBarrier.acceptHello(connection, 'restore-a')).toBe('ready');
  const serverDrafts = new Map<string, string>();
  let loseResponse = true;
  const effects = {
    allocateId: vi.fn(() => 'fresh-draft'),
    isCurrent: () => freshBarrier.isReady(connection),
    async createDraftIfAbsent(id: string, source: PreservedComposeIntent['drafts'][number]) {
      if (!serverDrafts.has(id)) serverDrafts.set(id, source.draft.text);
      if (loseResponse) { loseResponse = false; throw new Error('response lost'); }
    },
  };
  await expect(recoverRestoreDrafts(newStore, 'restore-a', effects)).rejects.toThrow('response lost');
  serverDrafts.set('fresh-draft', 'user edited after creation');
  expect(await recoverRestoreDrafts(newStore, 'restore-a', effects)).toEqual({ status: 'ready', recovered: 1 });
  expect(effects.allocateId).toHaveBeenCalledOnce();
  expect([...serverDrafts]).toEqual([['fresh-draft', 'user edited after creation']]);
  expect((await newStore.readRecoveries())[0].intent).toEqual(intent('before restore'));
});

it('recovers the edited descendant instead of resurrecting its original after another restore', async () => {
  const a = createRestoreGenerationStore('alpha', 'page-a');
  await a.preserve('restore-a', intent('original'));
  const original = (await a.readRecoveries())[0];
  await a.prepareRecovery(original, 'restore-a', () => 'recovered-a');
  // A lost acknowledgement is enough to leave completed empty. The durable
  // destination, rather than completion, establishes the content's lineage.
  const b = createRestoreGenerationStore('alpha', 'page-b');
  await b.observe('restore-b');
  await b.prepareRecovery(original, 'restore-b', () => 'prepared-before-capture');
  const edited = intent('edited recovered draft');
  edited.drafts[0].threadId = 'recovered-a';
  await b.preserve('restore-b', edited);
  expect((await b.prepareRecovery(original, 'restore-b', () => 'must-not-resurrect'))?.drafts).toEqual([]);
  const descendant = (await b.readRecoveries())[1];
  expect((await b.prepareRecovery(descendant, 'restore-b', () => 'recovered-b'))?.drafts)
    .toEqual([{ sourceThreadId: 'recovered-a', threadId: 'recovered-b' }]);
  // Repeat the lineage through a third generation and preserve all originals.
  const c = createRestoreGenerationStore('alpha', 'page-c');
  const latest = intent('edited twice');
  latest.drafts[0].threadId = 'recovered-b';
  await c.preserve('restore-c', latest);
  expect((await c.prepareRecovery(original, 'restore-c', () => 'unused'))?.drafts).toEqual([]);
  expect((await c.prepareRecovery(descendant, 'restore-c', () => 'unused'))?.drafts).toEqual([]);
  expect((await c.readRecoveries()).map((r) => r.intent.drafts[0].draft.text))
    .toEqual(['original', 'edited recovered draft', 'edited twice']);
  const created = vi.fn(async (_id: string, source: PreservedComposeIntent['drafts'][number]) => {
    expect(source.draft.text).toBe('edited twice');
  });
  expect(await recoverRestoreDrafts(c, 'restore-c', {
    allocateId: () => 'latest-destination', isCurrent: () => true, createDraftIfAbsent: created,
  })).toEqual({ status: 'ready', recovered: 1 });
  expect(created).toHaveBeenCalledOnce();
});

it('shares one unsent destination across independent captures by competing pages', async () => {
  const a = createRestoreGenerationStore('alpha', 'page-a');
  const b = createRestoreGenerationStore('alpha', 'page-b');
  const capture = intent('unsent request');
  capture.drafts[0].unsentMessage = { eventId: 'same-send' } as NonNullable<typeof capture.drafts[0]['unsentMessage']>;
  await Promise.all([a.preserve('restore-a', capture), b.preserve('restore-a', capture)]);
  const [first, second] = await a.readRecoveries();
  const [da, db] = await Promise.all([
    a.prepareRecovery(first, 'restore-a', () => 'only-destination'),
    b.prepareRecovery(second, 'restore-a', () => 'duplicate'),
  ]);
  expect(da?.drafts[0].threadId).toBe('only-destination');
  expect(db?.drafts[0].threadId).toBe('only-destination');
  await a.completeRecoveryDraft(first, 'restore-a', 'only-destination');
  expect((await b.prepareRecovery(second, 'restore-a', () => 'unused'))?.completed).toEqual(['only-destination']);
  const c = createRestoreGenerationStore('alpha', 'page-c');
  const edited = intent('current review edit');
  edited.drafts[0].threadId = 'only-destination';
  await c.preserve('restore-b', edited);
  expect((await c.prepareRecovery(first, 'restore-b', () => 'unused'))?.drafts).toEqual([]);
  expect((await c.prepareRecovery(second, 'restore-b', () => 'unused'))?.drafts).toEqual([]);
  const created = vi.fn(async (_id: string, source: PreservedComposeIntent['drafts'][number]) => {
    expect(source.draft.text).toBe('current review edit');
  });
  expect(await recoverRestoreDrafts(c, 'restore-b', {
    allocateId: () => 'current-destination', isCurrent: () => true, createDraftIfAbsent: created,
  })).toEqual({ status: 'ready', recovered: 1 });
});

it('defers live and unknown foreign owners, then shares an orphan destination across captures', async () => {
  const a = createRestoreGenerationStore('alpha', 'observer-a');
  const b = createRestoreGenerationStore('alpha', 'observer-b');
  const captured = intent('');
  captured.captureOwnerId = 'observer-a';
  captured.drafts[0].composerSource = { ownerId: 'owner', threadId: 'original' };
  await a.preserve('restore', captured);
  await b.preserve('restore', { ...captured, captureOwnerId: 'observer-b' });
  const [first, second] = await a.readRecoveries();
  const allocate = vi.fn(() => 'orphan-destination');
  expect((await a.prepareRecovery(first, 'restore', allocate, new Set(['owner'])))?.drafts).toEqual([]);
  expect((await a.prepareRecovery(first, 'restore', allocate, null))?.drafts).toEqual([]);
  expect(allocate).not.toHaveBeenCalled();
  const [recovered, peer] = await Promise.all([
    a.prepareRecovery(first, 'restore', allocate, new Set()),
    b.prepareRecovery(second, 'restore', allocate, new Set()),
  ]);
  expect(recovered?.drafts[0].threadId).toBe('orphan-destination');
  expect(peer?.drafts).toEqual(recovered?.drafts);
  await a.completeRecoveryDraft(first, 'restore', 'orphan-destination');
  expect((await b.prepareRecovery(second, 'restore', allocate, new Set()))?.completed).toEqual(['orphan-destination']);
  expect(allocate).toHaveBeenCalledTimes(1);
});

it('uses the owning page capture instead of foreign stubs, including a cleared composer', async () => {
  const observer = createRestoreGenerationStore('alpha', 'observer');
  const owner = createRestoreGenerationStore('alpha', 'owner');
  const stub = intent('');
  stub.captureOwnerId = 'observer';
  stub.drafts[0].composerSource = { ownerId: 'owner', threadId: 'original' };
  await observer.preserve('restore', stub);
  const actual = intent('latest edit');
  actual.captureOwnerId = 'owner';
  actual.drafts[0].composerSource = { ownerId: 'owner', threadId: 'original' };
  await owner.preserve('restore', actual);
  const [foreign, authoritative] = await owner.readRecoveries();
  const allocate = vi.fn(() => 'latest-destination');
  expect((await observer.prepareRecovery(foreign, 'restore', allocate, new Set()))?.drafts).toEqual([]);
  expect((await owner.prepareRecovery(authoritative, 'restore', allocate, null))?.drafts).toHaveLength(1);
  expect(allocate).toHaveBeenCalledTimes(1);
  const cleared = createRestoreGenerationStore('cleared', 'owner');
  const other = createRestoreGenerationStore('cleared', 'observer');
  await other.preserve('restore', stub);
  await cleared.preserve('restore', { captureOwnerId: 'owner', drafts: [], pendingSelection: {} });
  expect((await other.prepareRecovery((await other.readRecoveries())[0], 'restore', allocate, new Set()))?.drafts).toEqual([]);
});

it('shares the exact queued request destination across a foreign capture and its owning composer', async () => {
  const a = createRestoreGenerationStore('alpha', 'observer');
  const b = createRestoreGenerationStore('alpha', 'owner');
  const queued = { threadId: 'original', ownerId: 'owner', text: 'queued', localIds: ['image'],
    intent: { useCodingAgent: false, context: null } };
  const foreign = intent('queued');
  foreign.captureOwnerId = 'observer';
  foreign.drafts[0].queuedUploadSend = queued;
  const actual = intent('queued');
  actual.captureOwnerId = 'owner';
  actual.drafts[0].queuedUploadSend = queued;
  actual.drafts[0].composerSource = { ownerId: 'owner', threadId: 'original' };
  await a.preserve('restore', foreign);
  await b.preserve('restore', actual);
  const [first, second] = await a.readRecoveries();
  const allocate = vi.fn(() => 'shared-queue');
  const [one, two] = await Promise.all([
    a.prepareRecovery(first, 'restore', allocate, null),
    b.prepareRecovery(second, 'restore', allocate, null),
  ]);
  expect(one?.drafts[0].threadId).toBe('shared-queue');
  expect(two?.drafts[0].threadId).toBe('shared-queue');
  expect(allocate).toHaveBeenCalledTimes(1);
});

it('journals late sources atomically across reconnects and competing tabs without replacing owner capture', async () => {
  const { captureHistoricalSources } = await import('./restoreHistoricalSources');
  const store = createRestoreGenerationStore('late', 'owner');
  await store.preserve('restored', { captureOwnerId: 'owner', drafts: [], pendingSelection: {} });
  const upload = { localId: 'file', ownerId: 'owner', threadId: 'old', bytes: new Uint8Array([7]).buffer } as import('./pendingUploadRecords').PendingUploadRecord;
  const batch = await captureHistoricalSources('restored', { uploads: [upload], queuedUploadSendRecords: [] }, []);
  await Promise.all([store.journalHistorical('restored', batch, 'batch-a'), store.journalHistorical('restored', batch, 'batch-b')]);
  await store.journalHistorical('restored', batch, 'reconnect');
  const records = await store.readRecoveries();
  expect(records).toHaveLength(2);
  expect(records[0].intent.captureOwnerId).toBe('owner');
  expect(records[1].intent.captureOwnerId).toBeUndefined();
  expect(records[1].intent.drafts[0].uploads).toEqual([upload]);
  expect((await store.prepareRecovery(records[1], 'restored', () => 'unused', new Set()))?.drafts).toEqual([]);
});

it('keeps late bytes behind an already retained queue and refuses obsolete generations', async () => {
  const { captureHistoricalSources } = await import('./restoreHistoricalSources');
  const store = createRestoreGenerationStore('late', 'page');
  await store.observe('restored');
  const queue = { ownerId: 'other', threadId: 'old', text: 'queued', intent: { useCodingAgent: false, context: null }, localIds: ['file'] };
  await store.journalHistorical('restored', await captureHistoricalSources('restored', { uploads: [], queuedUploadSendRecords: [queue] }, []), 'queue');
  const upload = { localId: 'file', ownerId: 'other', threadId: 'old' } as import('./pendingUploadRecords').PendingUploadRecord;
  const late = await captureHistoricalSources('restored', { uploads: [upload], queuedUploadSendRecords: [queue] }, []);
  device.failNextCommit = true;
  await expect(store.journalHistorical('restored', late, 'late')).rejects.toThrow('transaction aborted');
  expect(await store.readRecoveries()).toHaveLength(1);
  await store.journalHistorical('restored', late, 'retry');
  expect((await store.readRecoveries())[1].intent.drafts[0].uploads).toEqual([upload]);
  await store.observe('newer');
  await expect(store.journalHistorical('restored', late, 'stale')).rejects.toThrow('generation changed');
  expect(await store.readGeneration()).toBe('newer');
});


it('retains a late upload version after completion without duplicating it on concurrent reconnect', async () => {
  const { captureHistoricalSources } = await import('./restoreHistoricalSources');
  const store = createRestoreGenerationStore('versions', 'observer');
  await store.observe('restored');
  const upload = { localId: 'file', ownerId: 'owner', threadId: 'old', sourceGeneration: 'old',
    name: 'photo', mime: 'image/png', size: 1, createdAt: 1, bytes: new Uint8Array([1]).buffer };
  const capture = (uploads: typeof upload[]) => captureHistoricalSources('restored', { uploads, queuedUploadSendRecords: [] }, []);
  await store.journalHistorical('restored', await capture([upload]), 'original');
  const original = (await store.readRecoveries())[0];
  const prepared = await store.prepareRecovery(original, 'restored', () => 'original-destination', new Set());
  await store.completeRecoveryDraft(original, 'restored', prepared!.drafts[0].threadId);
  const adopted = { ...upload, ownerId: 'adopter' };
  const newer = { ...upload, sourceGeneration: 'newer', bytes: new Uint8Array([2]).buffer };
  const batch = await capture([upload, adopted, newer]);
  device.failNextCommit = true;
  await expect(store.journalHistorical('restored', batch, 'failed')).rejects.toThrow('transaction aborted');
  expect(await store.readRecoveries()).toHaveLength(1);
  await Promise.all([store.journalHistorical('restored', batch, 'a'), store.journalHistorical('restored', batch, 'b')]);
  const records = await store.readRecoveries();
  expect(records).toHaveLength(2);
  expect(records[1].intent.drafts.flatMap((d) => d.uploads ?? [])).toEqual([adopted, newer]);
  const late = await store.prepareRecovery(records[1], 'restored', () => crypto.randomUUID(), new Set());
  expect(late?.drafts).toHaveLength(2);
  expect(late?.drafts[0].threadId).toBe('original-destination');
  expect(late?.drafts[1].threadId).not.toBe('original-destination');
  expect(late?.completed).toEqual(['original-destination']);
});

it('shares only exact standalone upload lineage across concurrent recovery allocations', async () => {
  const { captureHistoricalSources } = await import('./restoreHistoricalSources');
  const store = createRestoreGenerationStore('content-lineage', 'observer');
  await store.observe('restored');
  const upload = { localId: 'file', ownerId: 'owner', threadId: 'old', sourceGeneration: 'old',
    name: 'photo', mime: 'image/png', size: 1, createdAt: 1, bytes: new Uint8Array([1]).buffer };
  const versions = [upload, { ...upload, ownerId: 'adopter' }, { ...upload, landedHash: 'landed' },
    { ...upload, sourceGeneration: 'older', bytes: new Uint8Array([2]).buffer },
    { ...upload, localId: 'independent' }, { ...upload, mime: 'image/jpeg' }];
  await store.journalHistorical('restored', await captureHistoricalSources('restored', {
    uploads: versions, queuedUploadSendRecords: [],
  }, []), 'batch');
  const record = (await store.readRecoveries())[0];
  const [a, b] = await Promise.all([store.prepareRecovery(record, 'restored', () => crypto.randomUUID(), new Set()),
    store.prepareRecovery(record, 'restored', () => crypto.randomUUID(), new Set())]);
  expect(a?.drafts).toEqual(b?.drafts);
  const retained = (await store.readRecoveries())[0];
  expect(retained.materializations?.[0].drafts).toHaveLength(6);
  expect(new Set(retained.materializations?.[0].drafts.slice(0, 3).map((d) => d.threadId)).size).toBe(1);
  expect(a?.drafts).toHaveLength(4);
  expect(new Set(a?.drafts.map((d) => d.threadId)).size).toBe(4);
  const { recoverRestoreDrafts } = await import('./restoreDraftRecovery');
  const createDraftIfAbsent = vi.fn(async () => {});
  const effects = { allocateId: () => crypto.randomUUID(), isCurrent: () => true,
    readLiveOwners: async () => new Set<string>(), createDraftIfAbsent };
  expect(await recoverRestoreDrafts(store, 'restored', effects)).toEqual({ status: 'ready', recovered: 4 });
  expect(createDraftIfAbsent).toHaveBeenCalledTimes(4);
  expect(await recoverRestoreDrafts(store, 'restored', effects)).toEqual({ status: 'ready', recovered: 0 });
  expect(createDraftIfAbsent).toHaveBeenCalledTimes(4);
  expect((await store.readRecoveries())[0].intent.drafts.flatMap((d) => d.uploads ?? [])).toEqual(versions);
});


it('explicit review copies only the selected retained original despite a live owner or authoritative empty capture', async () => {
  const store = createRestoreGenerationStore('review-device', 'foreign');
  const original = intent('retained original');
  original.drafts[0].composerSource = { ownerId: 'live-owner', threadId: 'composer' };
  original.drafts.push({ ...original.drafts[0], threadId: 'unselected' });
  await store.preserve('restored', original);
  const owner = createRestoreGenerationStore('review-device', 'owner');
  await owner.preserve('restored', { captureOwnerId: 'live-owner', drafts: [], pendingSelection: {} });
  const record = (await store.readRecoveries())[0];
  expect((await store.prepareRecovery(record, 'restored', () => 'automatic', new Set()))?.drafts).toEqual([]);
  const review = { sourceThreadId: original.drafts[0].threadId };
  const prepared = await store.prepareRecovery(record, 'restored', () => 'review-copy', null, review);
  expect(prepared?.drafts).toEqual([{ sourceThreadId: review.sourceThreadId, threadId: 'review-copy' }]);
  await store.completeRecoveryDraft(record, 'restored', 'review-copy', true);
  expect((await store.prepareRecovery(record, 'restored', () => 'duplicate', null, review))?.completed).toEqual(['review-copy']);
  expect((await store.readRecoveries())[0].intent).toEqual(original);
  expect((await store.prepareRecovery(record, 'restored', () => 'automatic', new Set()))?.drafts).toEqual([]);
});

it('serializes explicit review copies and fences stale or missing selections', async () => {
  const a = createRestoreGenerationStore('review-device', 'a');
  const b = createRestoreGenerationStore('review-device', 'b');
  await a.preserve('restored', intent('original'));
  const record = (await a.readRecoveries())[0];
  const review = { sourceThreadId: record.intent.drafts[0].threadId };
  const [first, second] = await Promise.all([
    a.prepareRecovery(record, 'restored', () => 'one', null, review),
    b.prepareRecovery(record, 'restored', () => 'two', null, review),
  ]);
  expect(first).toEqual(second);
  await expect(a.prepareRecovery(record, 'restored', () => 'missing', null, { sourceThreadId: 'missing' })).rejects.toThrow('missing');
  await a.observe('next');
  expect(await a.prepareRecovery(record, 'restored', () => 'stale', null, review)).toBeUndefined();
  await expect(a.completeRecoveryDraft(record, 'restored', first!.drafts[0].threadId, true)).rejects.toThrow('current');
});

it('keeps reviewed copies separate from automatic destinations and preserves edits after a lost response', async () => {
  const store = createRestoreGenerationStore('review-device', 'page');
  await store.preserve('restored', intent('original'));
  const record = (await store.readRecoveries())[0];
  await store.prepareRecovery(record, 'restored', () => 'automatic');
  await store.completeRecoveryDraft(record, 'restored', 'automatic');
  const server = new Map([['automatic', 'edited automatic draft']]);
  let loseResponse = true;
  const createDraftIfAbsent = vi.fn(async (id: string, source: PreservedComposeIntent['drafts'][number]) => {
    if (!server.has(id)) server.set(id, source.draft.text);
    if (loseResponse) { loseResponse = false; throw new Error('lost response'); }
  });
  const { copyRestoreSourceForReview, readRestoreSourceReview } = await import('./restoreSourceActions');
  const reference = { pageId: record.pageId, generation: record.generation, sourceThreadId: record.intent.drafts[0].threadId };
  const effects = { allocateId: () => 'reviewed', isCurrent: () => true, createDraftIfAbsent };
  await expect(copyRestoreSourceForReview(store, 'restored', reference, effects)).rejects.toThrow('lost response');
  server.set('reviewed', 'edited copy after lost response');
  expect(await copyRestoreSourceForReview(store, 'restored', reference, effects)).toBe('reviewed');
  expect(await copyRestoreSourceForReview(store, 'restored', reference, effects)).toBe('reviewed');
  expect(createDraftIfAbsent).toHaveBeenCalledTimes(2);
  expect(server).toEqual(new Map([['automatic', 'edited automatic draft'], ['reviewed', 'edited copy after lost response']]));
  const review = await readRestoreSourceReview(store, effects);
  expect(review[0].destinations).toEqual([
    { generation: 'restored', threadId: 'automatic', completed: true },
    { generation: 'restored', threadId: 'reviewed', completed: true, reviewed: true },
  ]);
  expect((await store.readRecoveries())[0].intent).toEqual(record.intent);
});

it('recognizes explicit copies as descendants for later automatic recovery', async () => {
  const store = createRestoreGenerationStore('review-device', 'original-page');
  await store.preserve('first', intent('original'));
  const original = (await store.readRecoveries())[0];
  const sourceThreadId = original.intent.drafts[0].threadId;
  await store.prepareRecovery(original, 'first', () => 'review-copy', null, { sourceThreadId });
  await store.completeRecoveryDraft(original, 'first', 'review-copy', true);
  const automatic = await store.prepareRecovery(original, 'first', () => 'duplicate');
  expect(automatic?.drafts).toEqual([{ sourceThreadId, threadId: 'review-copy' }]);
  expect(automatic?.completed).toEqual(['review-copy']);
  const nextPage = createRestoreGenerationStore('review-device', 'next-page');
  const edited = intent('edited explicit copy');
  edited.drafts[0].threadId = 'review-copy';
  await nextPage.preserve('second', edited);
  expect((await nextPage.prepareRecovery(original, 'second', () => 'resurrected'))?.drafts).toEqual([]);
  const descendant = (await nextPage.readRecoveries())[1];
  expect((await nextPage.prepareRecovery(descendant, 'second', () => 'next-copy'))?.drafts).toEqual([
    { sourceThreadId: 'review-copy', threadId: 'next-copy' },
  ]);
});
