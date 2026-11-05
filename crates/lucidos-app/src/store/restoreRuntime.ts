/** Production restore admission. A transport open alone is never permission
 * to load archived state, clean up local records or dispatch compose intent. */
import { installDirectRequestAdmission } from '@lucidos/restore-generation';
import { bindSourceGeneration, observeSourceGeneration, sourceGeneration } from '../utils/restoreSourceGeneration';
import { WORKSPACE_ID } from '../utils/basePath';
import { generateUuid } from '../utils/uuid';
import { installRestoreAdmission, preserveRestoreSources, queuedRestoreSourceId, restoreAdmissionChanged, waitForRestoreAdmission } from '../utils/restoreAdmission';
import { createRecoveredDraft } from '../api/client/threads';
import { queuedUploadSends } from './uploadSendIntents';
import { pendingUploads } from './pendingUploads';
import { unsentMessages } from './unsentMessages';
import { composeDrafts } from './composeDrafts';
import { composeSelections, pendingComposeSelection } from './composeSelections';
import { pageOwnerId, holdPageOwnerLock, liveOwnerIds } from './pageOwner';
import { livePendingUploadRecords, readPendingUploadStore } from './pendingUploadRecords';
import { liveUnsentMessageRecords, readUnsentMessageStore } from './unsentMessageRecords';
import { RestoreGenerationBarrier, type RestoreGeneration } from './restoreGeneration';
import { createRestoreGenerationStore } from './restoreGenerationRecords';
import { captureHistoricalSources } from './restoreHistoricalSources';
import { captureRestoreIntent } from './restoreIntentCapture';
import { recoverRestoreDrafts } from './restoreDraftRecovery';
import { copyRestoreSourceForReview, readRestoreSourceReview, type RestoreSourceReference } from './restoreSourceActions';

let runtime: ReturnType<typeof createRuntime> | undefined;

function createRuntime() {
  installDirectRequestAdmission(() => {
    const generation = bindSourceGeneration();
    return {
      get header() { return generation() ?? 'none'; },
      wait: waitForRestoreAdmission,
      assertCurrent() {
        if (sourceGeneration() !== generation()) throw new Error('Workspace restored; review intent before submitting again.');
      },
    };
  });
  holdPageOwnerLock();
  const store = createRestoreGenerationStore(WORKSPACE_ID, pageOwnerId);
  let ready = false;
  let epoch = 0;
  let waiters: Array<() => void> = [];
  const barrier = new RestoreGenerationBarrier(store, {
    capture: async () => {
      // Freeze DOM editing BEFORE any asynchronous drain, through the journal
      // commit and reload. No keystroke can land after the captured value.
      document.body.inert = true;
      return captureRestoreIntent({ ownerId: pageOwnerId, readUploads: readPendingUploadStore, readUnsent: readUnsentMessageStore,
        readComposers: async () => {
          // Read the page-local origin before any await. The hello being
          // captured belongs to the next generation, never these live sources.
          const origin = sourceGeneration();
          const drafts = structuredClone(composeDrafts.peek());
          const selections = structuredClone(composeSelections.peek());
          const pendingSelection = structuredClone(pendingComposeSelection.peek());
          const unsent = structuredClone([...new Map([...liveUnsentMessageRecords(),
            ...[...unsentMessages.peek()].map(([eventId, message]) => ({ ...message,
              eventId, ownerId: pageOwnerId, sourceGeneration: origin, sentAt: new Date().toISOString(), phase: 'unsent' as const })),
          ].map((r) => [r.eventId, r])).values()]);
          const entries = [...pendingUploads.peek().values()].flat();
          const queued = structuredClone([...queuedUploadSends.peek()].map(([threadId, intent]) => ({ threadId, intent,
            ownerId: pageOwnerId, sourceGeneration: origin, localIds: entries.filter((u) => u.threadId === threadId).map((u) => u.localId),
            text: drafts.get(threadId)?.text ?? '' })));
          const retainedUploads = structuredClone(livePendingUploadRecords());
          const pendingFiles = await Promise.all(entries.map(async (entry) => ({
            localId: entry.localId, threadId: entry.threadId, name: entry.file.name, mime: entry.mime,
            landedHash: entry.contentHash,
            bytes: await entry.file.arrayBuffer(), size: entry.file.size, createdAt: Date.now(), ownerId: pageOwnerId, sourceGeneration: origin,
          })));
          const uploads = [...new Map([...retainedUploads, ...pendingFiles].map((r) => [r.localId, r])).values()];
          return { drafts, selections, pendingSelection, uploads, unsent, queued };
        } });
    },
    reload: () => window.location.reload(),
  });
  installRestoreAdmission({ ready: () => ready, wait: async () => {
    while (!ready) await new Promise<void>((resolve) => { waiters.push(resolve); });
  } });
  return {
    async reviewSources() {
      const connection = epoch;
      return readRestoreSourceReview(store, {
        isCurrent: () => ready && epoch === connection && barrier.isReady(connection),
        readLiveOwners: liveOwnerIds,
      });
    },
    async copySource(reference: RestoreSourceReference) {
      const connection = epoch;
      const generation = sourceGeneration();
      if (!ready || generation === null) throw new Error('Restore review is not ready');
      return copyRestoreSourceForReview(store, generation, reference, {
        allocateId: generateUuid,
        isCurrent: () => ready && epoch === connection && barrier.isReady(connection),
        createDraftIfAbsent: (id, source) => createRecoveredDraft(id, generation, source),
      });
    },
    begin() { ready = false; epoch = barrier.beginConnection(); restoreAdmissionChanged(); return epoch; },
    async hello(connection: number, generation: RestoreGeneration): Promise<boolean> {
      let admission;
      try {
        admission = await barrier.acceptHello(connection, generation);
      } catch (error) {
        // Capture froze editing for a reload that will not come now.
        document.body.inert = false;
        throw error;
      }
      if (admission !== 'ready') return false;
      const current = () => connection === epoch && barrier.isReady(connection);
      const [pending, unsent] = await Promise.all([readPendingUploadStore(true), readUnsentMessageStore()]);
      if (!current()) return false;
      const historical = await captureHistoricalSources(generation, pending, unsent);
      if (!current()) return false;
      await store.journalHistorical(generation, historical, generateUuid());
      if (!current()) return false;
      const records = await store.readRecoveries();
      if (!current()) return false;
      preserveRestoreSources(records.flatMap((r) => r.intent.drafts.flatMap((d) => d.uploads?.map((u) => u.localId) ?? [])),
        records.flatMap((r) => r.intent.unsentMessages?.map((s) => s.eventId) ?? []),
        records.flatMap((r) => r.intent.queuedUploadSends?.map(queuedRestoreSourceId) ?? []));
      if (generation !== null) {
        const result = await recoverRestoreDrafts(store, generation, { allocateId: generateUuid, isCurrent: current, readLiveOwners: liveOwnerIds,
          createDraftIfAbsent: (id, source) => createRecoveredDraft(id, generation, source) });
        if (result.status !== 'ready') return false;
      }
      if (!current()) return false;
      observeSourceGeneration(generation, barrier.baselineFromPageLoad);
      ready = true;
      restoreAdmissionChanged();
      const released = waiters;
      waiters = [];
      for (const resolve of released) resolve();
      return true;
    },
  };
}

export function installRestoreRuntime(): void { runtime ??= createRuntime(); }
export function hasRestoreRuntime(): boolean { return runtime !== undefined; }
export function beginRestoreConnection(): number { return runtime?.begin() ?? 0; }
export async function acceptRestoreHello(connection: number, generation: RestoreGeneration): Promise<boolean> {
  return await runtime?.hello(connection, generation) ?? true;
}

/** Explicit review re-reads owner liveness, including owners that died after
 * admission. It never automatically reconstructs or sends the listed sources. */
export async function readRetainedRestoreSources() {
  if (!runtime) throw new Error('Restore review is not ready');
  return runtime.reviewSources();
}

export async function copyRetainedRestoreSource(reference: RestoreSourceReference): Promise<string> {
  if (!runtime) throw new Error('Restore review is not ready');
  return runtime.copySource(reference);
}
