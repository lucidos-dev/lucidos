import { describe, expect, it, vi } from 'vitest';
import { recoverRestoreDrafts, type RestoreDraftRecoveryEffects } from './restoreDraftRecovery';
import type { RestoreIntentStore } from './restoreGenerationRecords';

function setup() {
  let ready = true;
  const source = {
    pageId: 'old-page', generation: 'restored',
    intent: {
      drafts: [{ threadId: 'archived', draft: { text: 'review this', image_hashes: ['image'], mode: 'lucidos' as const }, selection: { model: 'chosen' } }],
      pendingSelection: { codingAgent: 'codex' as const },
    },
  };
  const store = {
    readRecoveries: vi.fn(async () => [source]),
    prepareRecovery: vi.fn(async () => ({ generation: 'restored', drafts: [{ sourceThreadId: 'archived', threadId: 'fresh' }], completed: [] as string[] })),
    completeRecoveryDraft: vi.fn(async () => {}),
  } as unknown as RestoreIntentStore;
  const effects = {
    allocateId: () => 'allocated',
    isCurrent: () => ready,
    createDraftIfAbsent: vi.fn<RestoreDraftRecoveryEffects['createDraftIfAbsent']>(async () => {}),
  };
  return { source, store, effects, revoke: () => { ready = false; } };
}

describe('restore draft reconstruction', () => {
  it('uses durable destinations and preserves text, images, mode and selection without sending', async () => {
    const { source, store, effects } = setup();
    expect(await recoverRestoreDrafts(store, 'restored', effects)).toEqual({ status: 'ready', recovered: 1 });
    expect(effects.createDraftIfAbsent).toHaveBeenCalledExactlyOnceWith('fresh', source.intent.drafts[0]);
    expect(store.completeRecoveryDraft).toHaveBeenCalledWith(source, 'restored', 'fresh');
  });

  it('retries a failed server write without acknowledging or modifying the capture', async () => {
    const { store, effects } = setup();
    effects.createDraftIfAbsent.mockRejectedValueOnce(new Error('offline'));
    await expect(recoverRestoreDrafts(store, 'restored', effects)).rejects.toThrow('offline');
    expect(store.completeRecoveryDraft).not.toHaveBeenCalled();
    await recoverRestoreDrafts(store, 'restored', effects);
    expect(effects.createDraftIfAbsent.mock.calls.map((args) => args[0])).toEqual(['fresh', 'fresh']);
  });

  it('does not replay completed drafts or overwrite subsequent user edits', async () => {
    const { store, effects } = setup();
    vi.mocked(store.prepareRecovery).mockResolvedValue({ generation: 'restored', drafts: [{ sourceThreadId: 'archived', threadId: 'fresh' }], completed: ['fresh'] });
    expect(await recoverRestoreDrafts(store, 'restored', effects)).toEqual({ status: 'ready', recovered: 0 });
    expect(effects.createDraftIfAbsent).not.toHaveBeenCalled();
  });

  it('stops when the connection changes while reading the journal', async () => {
    const { store, effects, revoke } = setup();
    const original = vi.mocked(store.readRecoveries).getMockImplementation()!;
    vi.mocked(store.readRecoveries).mockImplementation(async () => { const records = await original(); revoke(); return records; });
    expect(await recoverRestoreDrafts(store, 'restored', effects)).toEqual({ status: 'stale', recovered: 0 });
    expect(store.prepareRecovery).not.toHaveBeenCalled();
  });

  it('does not acknowledge an old response after the lease expires', async () => {
    const { store, effects, revoke } = setup();
    effects.createDraftIfAbsent.mockImplementation(async () => { revoke(); });
    expect(await recoverRestoreDrafts(store, 'restored', effects)).toEqual({ status: 'stale', recovered: 0 });
    expect(store.completeRecoveryDraft).not.toHaveBeenCalled();
  });

  it('stops when another tab advances the durable generation', async () => {
    const { store, effects } = setup();
    vi.mocked(store.prepareRecovery).mockResolvedValue(undefined);
    expect(await recoverRestoreDrafts(store, 'restored', effects)).toEqual({ status: 'stale', recovered: 0 });
    expect(effects.createDraftIfAbsent).not.toHaveBeenCalled();
  });
});

it('checks the connection lease after querying owner liveness', async () => {
  const { store, effects, revoke } = setup();
  const readLiveOwners = vi.fn(async () => { revoke(); return new Set<string>(); });
  expect(await recoverRestoreDrafts(store, 'restored', { ...effects, readLiveOwners }))
    .toEqual({ status: 'stale', recovered: 0 });
  expect(store.prepareRecovery).not.toHaveBeenCalled();
  expect(effects.createDraftIfAbsent).not.toHaveBeenCalled();
});
