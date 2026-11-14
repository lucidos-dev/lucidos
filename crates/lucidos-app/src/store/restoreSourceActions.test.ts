import { describe, expect, it, vi } from 'vitest';
import { copyRestoreSourceForReview, readRestoreSourceReview } from './restoreSourceActions';
import type { RestoreDraftRecoveryEffects } from './restoreDraftRecovery';
import type { RestoreIntentStore, RestoreIntentRecord } from './restoreGenerationRecords';

function setup() {
  let current = true;
  const record: RestoreIntentRecord = { pageId: 'page', generation: 'old', intent: {
    drafts: [{ threadId: 'source', composerSource: { ownerId: 'foreign', threadId: 'original' },
      draft: { text: 'retained original', image_hashes: [], mode: null },
      selection: { model: 'original-model', scope: { kind: 'lucidos' } } }],
    pendingSelection: { codingAgent: 'codex' },
  } };
  const reference = { pageId: 'page', generation: 'old', sourceThreadId: 'source' };
  const store = {
    readRecoveries: vi.fn(async () => [record]),
    prepareRecovery: vi.fn(async () => ({ generation: 'current', drafts: [{ sourceThreadId: 'source', threadId: 'copy' }], completed: [] as string[] })),
    completeRecoveryDraft: vi.fn(async () => {}),
  } as unknown as RestoreIntentStore;
  const effects = { allocateId: () => 'allocated', isCurrent: () => current,
    readLiveOwners: vi.fn(async () => null),
    createDraftIfAbsent: vi.fn<RestoreDraftRecoveryEffects['createDraftIfAbsent']>(async () => {}) };
  return { record, reference, store, effects, revoke: () => { current = false; } };
}

describe('explicit restore source review', () => {
  it('refreshes unknown owner liveness on request without reinterpreting it as orphaned', async () => {
    const { store, effects, record } = setup();
    const review = await readRestoreSourceReview(store, effects);
    expect(review[0]).toMatchObject({ ownership: 'unknown', automatic: false, source: record.intent.drafts[0] });
    expect(effects.readLiveOwners).toHaveBeenCalledOnce();
    expect(store.prepareRecovery).not.toHaveBeenCalled();
  });

  it('copies exactly the durable selected source, preserving context without changing the original', async () => {
    const { store, effects, record, reference } = setup();
    const original = structuredClone(record);
    expect(await copyRestoreSourceForReview(store, 'current', reference, effects)).toBe('copy');
    expect(store.prepareRecovery).toHaveBeenCalledWith(record, 'current', effects.allocateId, null, { sourceThreadId: 'source' });
    expect(effects.createDraftIfAbsent).toHaveBeenCalledExactlyOnceWith('copy', record.intent.drafts[0]);
    expect(store.completeRecoveryDraft).toHaveBeenCalledWith(record, 'current', 'copy', true);
    expect(record).toEqual(original);
  });

  it('retains a lost-response destination for retry and skips acknowledged copies', async () => {
    const { store, effects, reference } = setup();
    effects.createDraftIfAbsent.mockRejectedValueOnce(new Error('lost response'));
    await expect(copyRestoreSourceForReview(store, 'current', reference, effects)).rejects.toThrow('lost response');
    expect(store.completeRecoveryDraft).not.toHaveBeenCalled();
    expect(await copyRestoreSourceForReview(store, 'current', reference, effects)).toBe('copy');
    vi.mocked(store.prepareRecovery).mockResolvedValue({ generation: 'current', drafts: [{ sourceThreadId: 'source', threadId: 'copy' }], completed: ['copy'] });
    expect(await copyRestoreSourceForReview(store, 'current', reference, effects)).toBe('copy');
    expect(effects.createDraftIfAbsent).toHaveBeenCalledTimes(2);
  });

  it('refuses missing sources and stale generations before creating anything', async () => {
    const { store, effects, reference } = setup();
    await expect(copyRestoreSourceForReview(store, 'current', { ...reference, sourceThreadId: 'missing' }, effects)).rejects.toThrow('missing');
    expect(store.prepareRecovery).not.toHaveBeenCalled();
    vi.mocked(store.prepareRecovery).mockResolvedValue(undefined);
    await expect(copyRestoreSourceForReview(store, 'current', reference, effects)).rejects.toThrow('generation changed');
    expect(effects.createDraftIfAbsent).not.toHaveBeenCalled();
  });

  it('never acknowledges an old response after losing its connection lease', async () => {
    const { store, effects, reference, revoke } = setup();
    effects.createDraftIfAbsent.mockImplementation(async () => revoke());
    await expect(copyRestoreSourceForReview(store, 'current', reference, effects)).rejects.toThrow('connection changed');
    expect(store.completeRecoveryDraft).not.toHaveBeenCalled();
  });

  it('checks the lease after a review liveness refresh', async () => {
    const { store, effects, revoke } = setup();
    effects.readLiveOwners.mockImplementation(async () => { revoke(); return null; });
    await expect(readRestoreSourceReview(store, effects)).rejects.toThrow('connection changed');
  });
});
