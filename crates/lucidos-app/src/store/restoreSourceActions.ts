import type { RestoreGeneration } from './restoreGeneration';
import type { RestoreIntentStore } from './restoreGenerationRecords';
import type { RestoreDraftRecoveryEffects } from './restoreDraftRecovery';
import { restoreSourceReview } from './restoreSourceReview';

export interface RestoreSourceReference {
  pageId: string;
  generation: RestoreGeneration;
  sourceThreadId: string;
}

export async function readRestoreSourceReview(store: RestoreIntentStore,
  effects: Pick<RestoreDraftRecoveryEffects, 'isCurrent' | 'readLiveOwners'>) {
  const records = await store.readRecoveries();
  if (!effects.isCurrent()) throw new Error('Restore review connection changed');
  const owners = await effects.readLiveOwners?.() ?? null;
  if (!effects.isCurrent()) throw new Error('Restore review connection changed');
  return restoreSourceReview(records, owners);
}

export async function copyRestoreSourceForReview(store: RestoreIntentStore, generation: RestoreGeneration,
  reference: RestoreSourceReference, effects: RestoreDraftRecoveryEffects): Promise<string> {
  const current = () => {
    if (!effects.isCurrent()) throw new Error('Restore review connection changed');
  };
  current();
  const records = await store.readRecoveries();
  current();
  const record = records.find((r) => r.pageId === reference.pageId && r.generation === reference.generation);
  const source = record?.intent.drafts.find((d) => d.threadId === reference.sourceThreadId);
  if (!record || !source) throw new Error('Preserved restore source is missing');
  const materialization = await store.prepareRecovery(record, generation, effects.allocateId, null,
    { sourceThreadId: reference.sourceThreadId });
  current();
  const destination = materialization?.drafts.find((d) => d.sourceThreadId === reference.sourceThreadId);
  if (!destination) throw new Error('Restore review generation changed');
  if (materialization!.completed.includes(destination.threadId)) return destination.threadId;
  const draft = await store.materializeDraft(source);
  current();
  await effects.createDraftIfAbsent(destination.threadId, draft);
  current();
  await store.completeRecoveryDraft(record, generation, destination.threadId, true);
  current();
  return destination.threadId;
}
