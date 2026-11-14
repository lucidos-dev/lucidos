/** Reconstruct captured composers as reviewable drafts. The transport adapter
 * must create by the supplied destination id and must never send a message.
 * Kept independent of application signals so admission can run before startup. */
import type { RestoreGeneration } from './restoreGeneration';
import type { PreservedComposeIntent, RestoreIntentStore } from './restoreGenerationRecords';

export interface RestoreDraftRecoveryEffects {
  allocateId(): string;
  /** Null means liveness cannot be established; retain foreign composers. */
  readLiveOwners?(): Promise<ReadonlySet<string> | null>;
  /** Connection lease, checked after every asynchronous boundary. */
  isCurrent(): boolean;
  /** Atomically create content and selections only if the destination does not
   * exist. A retry MUST leave an existing draft untouched: the user may have
   * edited it after a lost response. Never implement as create then PUT. */
  createDraftIfAbsent(threadId: string, source: PreservedComposeIntent['drafts'][number]): Promise<void>;
}

export type RestoreDraftRecoveryOutcome = {
  status: 'ready' | 'stale';
  recovered: number;
};

export async function recoverRestoreDrafts(
  store: RestoreIntentStore,
  generation: RestoreGeneration,
  effects: RestoreDraftRecoveryEffects,
): Promise<RestoreDraftRecoveryOutcome> {
  let recovered = 0;
  const outcome = (status: 'ready' | 'stale'): RestoreDraftRecoveryOutcome => ({ status, recovered });
  if (!effects.isCurrent()) return outcome('stale');
  const records = await store.readRecoveries();
  if (!effects.isCurrent()) return outcome('stale');
  for (const record of records) {
    const liveOwners = await effects.readLiveOwners?.() ?? null;
    if (!effects.isCurrent()) return outcome('stale');
    const destinations = await store.prepareRecovery(record, generation, effects.allocateId, liveOwners);
    if (!effects.isCurrent() || !destinations) return outcome('stale');
    for (const destination of destinations.drafts) {
      if (destinations.completed.includes(destination.threadId)) continue;
      const source = record.intent.drafts.find((draft) => draft.threadId === destination.sourceThreadId);
      if (!source) throw new Error('Preserved restore draft is missing');
      await effects.createDraftIfAbsent(destination.threadId, source);
      if (!effects.isCurrent()) return outcome('stale');
      await store.completeRecoveryDraft(record, generation, destination.threadId);
      if (!effects.isCurrent()) return outcome('stale');
      recovered += 1;
    }
  }
  return outcome('ready');
}
