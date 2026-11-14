/** Device-local restore observations and compose intent. Separate from caches:
 * recovery records are retained until the user can review their new drafts. */
import { sameUploadContentLineage, uploadSourceVersion } from './pendingUploadSource';
import { historicalSourceId, historicalUploadDraft } from './restoreHistoricalSources';
import { queuedRestoreSourceId } from '../utils/restoreAdmission';
import { restoreSourceReview } from './restoreSourceReview';
import type { ComposeDraft } from './composeDrafts';
import type { ComposeSelectionOverride } from './composeSelections';
import type { PendingUploadRecord, QueuedUploadSendRecord } from './pendingUploadRecords';
import type { UnsentMessageRecord } from './unsentMessageRecords';
import { BOUNDED, deviceDatabaseName, openDeviceDatabase } from './deviceDatabase';
import type { GenerationExpectation, RestoreGeneration, RestoreGenerationStore } from './restoreGeneration';

export interface PreservedComposeIntent {
  /** Capturing an owner is authoritative even when its composer was cleared. */
  captureOwnerId?: string;
  drafts: Array<{
    threadId: string;
    composerSource?: { ownerId: string; threadId: string };
    draft: ComposeDraft;
    selection: ComposeSelectionOverride;
    /** Retained bytes, including uploads that had not landed at capture. */
    uploads?: PendingUploadRecord[];
    /** Original context and inline images, retained until explicitly reviewed. */
    unsentMessage?: UnsentMessageRecord;
    queuedUploadSend?: QueuedUploadSendRecord;
  }>;
  pendingSelection: ComposeSelectionOverride;
  /** Original sources stay available for review; never re-enqueued as sends. */
  unsentMessages?: UnsentMessageRecord[];
  queuedUploadSends?: QueuedUploadSendRecord[];
}

export interface RestoreIntentRecord {
  /** Stable for this page, so retrying a failed/lost commit cannot duplicate it. */
  pageId: string;
  generation: RestoreGeneration;
  intent: PreservedComposeIntent;
  materializations?: RestoreDraftMaterialization[];
  /** Explicit copies have their own destinations: reviewing an older original
   * must not reuse an automatically recovered draft the user has edited. */
  reviewedMaterializations?: RestoreDraftMaterialization[];
}

/** Destination ids commit before any server request. Retries upsert the same
 * draft; they never reinterpret the captured intent as a send or an approval. */
export interface RestoreDraftMaterialization {
  generation: RestoreGeneration;
  drafts: Array<{ sourceThreadId: string; threadId: string }>;
  completed: string[];
}

export interface RestoreIntentStore extends RestoreGenerationStore<PreservedComposeIntent> {
  journalHistorical(generation: RestoreGeneration, intent: PreservedComposeIntent, batchId: string): Promise<void>;
  readRecoveries(): Promise<RestoreIntentRecord[]>;
  prepareRecovery(source: Pick<RestoreIntentRecord, 'pageId' | 'generation'>,
    target: RestoreGeneration, allocateId: () => string, liveOwners?: ReadonlySet<string> | null, review?: { sourceThreadId: string }): Promise<RestoreDraftMaterialization | undefined>;
  completeRecoveryDraft(source: Pick<RestoreIntentRecord, 'pageId' | 'generation'>,
    target: RestoreGeneration, threadId: string, reviewed?: boolean): Promise<void>;
}

/** Only synthetic attachment stubs can share content destinations. A composer,
 * queued revision or uncertain send retains its own text and review context. */
function standaloneUpload(draft: PreservedComposeIntent['drafts'][number]): PendingUploadRecord | undefined {
  const upload = draft.uploads?.length === 1 ? draft.uploads[0] : undefined;
  return upload && !draft.unsentMessage && !draft.queuedUploadSend &&
    draft.threadId === historicalUploadDraft(upload).threadId ? upload : undefined;
}

interface Observation {
  generation: RestoreGeneration;
  recoveries: RestoreIntentRecord[];
}

export function restoreGenerationDbName(workspaceId: string | null): string {
  return deviceDatabaseName('lucidos-restore-generation', workspaceId);
}

const STORE = 'observations';
const KEY = 'workspace';

export function createRestoreGenerationStore(workspaceId: string | null, pageId: string):
RestoreIntentStore {
  const { transact } = openDeviceDatabase(restoreGenerationDbName(workspaceId), (db) => {
    db.createObjectStore(STORE);
  });
  const read = () => transact<Observation>(STORE, 'readonly', (s) => s.get(KEY), BOUNDED);
  async function write(generation: RestoreGeneration, intent?: PreservedComposeIntent, expected?: GenerationExpectation): Promise<void> {
    let conflicted = false;
    // Read/modify/write in ONE transaction: tabs cannot erase each other's
    // preserved intent, and an observation cannot commit without its capture.
    // Bounded, a preserve included: an aborted capture is captured again on
    // the retry, and the backoff gives a large one longer each time.
    await transact(STORE, 'readwrite', (s) => {
      const request = s.get(KEY);
      request.onsuccess = () => {
        const previous = request.result as Observation | undefined;
        // A late hello from another tab's old connection must not regress the
        // marker. A concurrent capture of the SAME target remains admissible.
        if (expected && previous?.generation !== expected.generation && previous?.generation !== generation) {
          conflicted = true;
          return;
        }
        const recoveries = previous?.recoveries ?? [];
        const alreadyPreserved = recoveries.some((r) => r.pageId === pageId && r.generation === generation);
        s.put({
          generation,
          recoveries: intent !== undefined && !alreadyPreserved
            ? [...recoveries, { pageId, generation, intent }]
            : recoveries,
        } satisfies Observation, KEY);
      };
    }, BOUNDED);
    if (conflicted) throw new Error('Restore generation changed in another tab');
  }
  return {
    readGeneration: async () => (await read())?.generation,
    readRecoveries: async () => (await read())?.recoveries ?? [],
    observe: (generation, expected) => write(generation, undefined, expected),
    preserve: (generation, intent, expected) => write(generation, intent, expected),
    async journalHistorical(generation, intent, batchId) {
      let accepted = false;
      await transact(STORE, 'readwrite', (s) => {
        const request = s.get(KEY);
        request.onsuccess = () => {
          const observation = request.result as Observation | undefined;
          if (!observation || observation.generation !== generation) return;
          accepted = true;
          const previous = observation.recoveries.flatMap((r) => r.intent.drafts);
          const known = new Set(previous.map(historicalSourceId));
          const knownUploads = new Set(previous.flatMap((d) => d.uploads?.map(uploadSourceVersion) ?? []));
          const knownSends = new Set(observation.recoveries.flatMap((r) => r.intent.unsentMessages?.map((u) => `send:${u.eventId}`) ?? []));
          const knownQueues = new Set(observation.recoveries.flatMap((r) => r.intent.queuedUploadSends?.map((q) => `queue:${queuedRestoreSourceId(q)}`) ?? []));
          const drafts = intent.drafts.filter((d) => !known.has(historicalSourceId(d)) &&
            !knownSends.has(historicalSourceId(d)) && !knownQueues.has(historicalSourceId(d)) &&
            (d.unsentMessage || d.queuedUploadSend || d.uploads?.some((u) => !knownUploads.has(uploadSourceVersion(u)))));
          const included = new Set(drafts.flatMap((d) => d.uploads?.map(uploadSourceVersion) ?? []));
          // A queue can have been captured before its last file write landed.
          // Keep those new bytes even when the queue itself is already retained.
          for (const upload of intent.drafts.flatMap((d) => d.uploads ?? [])) {
            if (knownUploads.has(uploadSourceVersion(upload)) || included.has(uploadSourceVersion(upload))) continue;
            included.add(uploadSourceVersion(upload));
            drafts.push(historicalUploadDraft(upload));
          }
          if (drafts.length === 0) return;
          observation.recoveries.push({ pageId: batchId, generation, intent: {
            drafts, pendingSelection: {},
            unsentMessages: drafts.flatMap((d) => d.unsentMessage ? [d.unsentMessage] : []),
            queuedUploadSends: drafts.flatMap((d) => d.queuedUploadSend ? [d.queuedUploadSend] : []),
          } });
          s.put(observation, KEY);
        };
      }, BOUNDED);
      if (!accepted) throw new Error('Restore generation changed before historical capture');
    },
    async prepareRecovery(source, target, allocateId, liveOwners = null, review) {
      let missing = false;
      let result: RestoreDraftMaterialization | undefined;
      await transact(STORE, 'readwrite', (s) => {
        const request = s.get(KEY);
        request.onsuccess = () => {
          const observation = request.result as Observation | undefined;
          if (!observation || observation.generation !== target) return;
          const record = observation.recoveries.find((r) => r.pageId === source.pageId && r.generation === source.generation);
          if (!record) return;
          if (review) {
            if (!record.intent.drafts.some((d) => d.threadId === review.sourceThreadId)) {
              missing = true;
              return;
            }
            record.reviewedMaterializations ??= [];
            let materialization = record.reviewedMaterializations.find((m) => m.generation === target);
            if (!materialization) {
              materialization = { generation: target, drafts: [], completed: [] };
              record.reviewedMaterializations.push(materialization);
            }
            let destination = materialization.drafts.find((d) => d.sourceThreadId === review.sourceThreadId);
            if (!destination) {
              destination = { sourceThreadId: review.sourceThreadId, threadId: allocateId() };
              materialization.drafts.push(destination);
            }
            // One click authorizes only this original, never its related sources.
            result = { ...materialization, drafts: [destination] };
            s.put(observation, KEY);
            return;
          }
          // A later capture of one of our durable destinations is the current
          // revision of that source, even if its creation acknowledgement was
          // lost. Keep the original for review, but never resurrect it beside
          // the user's edited descendant. Check all generations so the lineage
          // survives repeated restores without mutating retained source content.
          const sources = observation.recoveries.flatMap((r, index) => r.intent.drafts.map((draft) => ({ r, index, draft })));
          const destinations = (draft: PreservedComposeIntent['drafts'][number]) => sources
            .filter((s) => draft.unsentMessage
              ? s.draft.unsentMessage?.eventId === draft.unsentMessage.eventId
              : draft.queuedUploadSend
                ? s.draft.queuedUploadSend !== undefined &&
                  queuedRestoreSourceId(s.draft.queuedUploadSend) === queuedRestoreSourceId(draft.queuedUploadSend)
              : standaloneUpload(draft)
                ? standaloneUpload(s.draft) !== undefined &&
                  sameUploadContentLineage(standaloneUpload(draft)!, standaloneUpload(s.draft)!)
              : draft.composerSource
                ? s.draft.composerSource?.ownerId === draft.composerSource.ownerId &&
                  s.draft.composerSource?.threadId === draft.composerSource.threadId
                : s.r === record && s.draft.threadId === draft.threadId)
            .flatMap((s) => [...(s.r.materializations ?? []), ...(s.r.reviewedMaterializations ?? [])].flatMap((m) => m.drafts
              .filter((d) => d.sourceThreadId === s.draft.threadId)
              .map((d) => ({ ...d, generation: m.generation, index: s.index }))));
          const superseded = new Set(record.intent.drafts.filter((draft) => destinations(draft).some((d) =>
            observation.recoveries.slice(d.index + 1).some((r) => r.generation !== d.generation &&
              r.intent.drafts.some((current) => current.threadId === d.threadId))))
            .map((draft) => draft.threadId));
          // Foreign records are retained immediately, but only an authoritative
          // owner capture or a proven orphan may produce a composer. Unsupported
          // liveness remains unknown, never evidence that another page died.
          const eligible = restoreSourceReview(observation.recoveries, liveOwners)
            .filter((entry) => entry.record === record && entry.automatic && !superseded.has(entry.source.threadId))
            .map((entry) => entry.source);
          record.materializations ??= [];
          result = record.materializations.find((m) => m.generation === target);
          if (!result) {
            result = { generation: target, drafts: [], completed: [] };
            record.materializations.push(result);
          }
          for (const draft of eligible) {
            if (result.drafts.some((d) => d.sourceThreadId === draft.threadId)) continue;
            result.drafts.push({ sourceThreadId: draft.threadId,
              threadId: destinations(draft).find((m) => m.generation === target)?.threadId ?? allocateId() });
          }
          s.put(observation, KEY);
          // A competing page may have prepared this target before the edited
          // descendant was captured. Retain its allocation for audit/retries,
          // while excluding the superseded source from dispatch.
          result = { ...result, drafts: result.drafts.filter((d) => eligible.some((source) => source.threadId === d.sourceThreadId))
            .filter((d, index, drafts) => drafts.findIndex((candidate) => candidate.threadId === d.threadId) === index),
            completed: [...new Set(observation.recoveries.flatMap((r) => [...(r.materializations ?? []), ...(r.reviewedMaterializations ?? [])]
              .filter((m) => m.generation === target).flatMap((m) => m.completed)))]
              .filter((id) => result!.drafts.some((d) => d.threadId === id)) };
        };
      }, BOUNDED);
      if (missing) throw new Error('Preserved restore source is missing');
      return result;
    },
    async completeRecoveryDraft(source, target, threadId, reviewed = false) {
      let accepted = false;
      await transact(STORE, 'readwrite', (s) => {
        const request = s.get(KEY);
        request.onsuccess = () => {
          const observation = request.result as Observation | undefined;
          if (!observation || observation.generation !== target) return;
          const record = observation.recoveries.find((r) => r.pageId === source.pageId && r.generation === source.generation);
          const materialization = (reviewed ? record?.reviewedMaterializations : record?.materializations)?.find((m) => m.generation === target);
          if (!materialization?.drafts.some((d) => d.threadId === threadId)) return;
          accepted = true;
          if (!materialization.completed.includes(threadId)) {
            materialization.completed.push(threadId);
            s.put(observation, KEY);
          }
        };
      }, BOUNDED);
      if (!accepted) throw new Error('Restore draft destination is no longer current');
    },
  };
}
