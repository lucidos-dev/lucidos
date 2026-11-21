/** Device-local restore observations and compose intent. Separate from caches:
 * recovery records are retained until the user can review their new drafts. */
import { sameStoredUpload, sameUploadBytes, uploadSourceVersion,
  type StoredUploadSource, type UploadSourceMetadata } from './pendingUploadSource';
import { readUploadPayload, retainUploadBytes, UPLOAD_BYTES } from './uploadByteStore';
import { historicalSourceId, historicalUploadDraft } from './restoreHistoricalSources';
import { queuedRestoreSourceId } from '../utils/restoreAdmission';
import { restoreSourceReview } from './restoreSourceReview';
import type { ComposeDraft } from './composeDrafts';
import type { ComposeSelectionOverride } from './composeSelections';
import type { PendingUploadRecord, QueuedUploadSendRecord } from './pendingUploadRecords';
import type { UnsentMessageRecord } from './unsentMessageRecords';
import { BOUNDED, deviceDatabaseName, openDeviceDatabase } from './deviceDatabase';
import type { GenerationExpectation, RestoreGeneration, RestoreGenerationStore } from './restoreGeneration';

export interface PreservedComposeIntent<Upload extends UploadSourceMetadata = PendingUploadRecord> {
  /** Capturing an owner is authoritative even when its composer was cleared. */
  captureOwnerId?: string;
  drafts: Array<{
    threadId: string;
    composerSource?: { ownerId: string; threadId: string };
    draft: ComposeDraft;
    selection: ComposeSelectionOverride;
    /** Retained uploads, including those that had not landed at capture. */
    uploads?: Upload[];
    /** Original context and inline images, retained until explicitly reviewed. */
    unsentMessage?: UnsentMessageRecord;
    queuedUploadSend?: QueuedUploadSendRecord;
  }>;
  pendingSelection: ComposeSelectionOverride;
  /** Original sources stay available for review; never re-enqueued as sends. */
  unsentMessages?: UnsentMessageRecord[];
  queuedUploadSends?: QueuedUploadSendRecord[];
}

type ComposeDraftSource<Upload extends UploadSourceMetadata> = PreservedComposeIntent<Upload>['drafts'][number];

export interface RestoreIntentRecord<Upload extends UploadSourceMetadata = StoredUploadSource> {
  /** Stable for this page, so retrying a failed/lost commit cannot duplicate it. */
  pageId: string;
  generation: RestoreGeneration;
  intent: PreservedComposeIntent<Upload>;
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
  journalHistorical(generation: RestoreGeneration, intent: PreservedComposeIntent<StoredUploadSource>,
    batchId: string, readBytes: (source: StoredUploadSource) => Promise<ArrayBuffer | undefined>): Promise<void>;
  readRecoveries(): Promise<RestoreIntentRecord[]>;
  materializeDraft(draft: ComposeDraftSource<StoredUploadSource>): Promise<ComposeDraftSource<PendingUploadRecord>>;
  prepareRecovery(source: Pick<RestoreIntentRecord, 'pageId' | 'generation'>,
    target: RestoreGeneration, allocateId: () => string, liveOwners?: ReadonlySet<string> | null, review?: { sourceThreadId: string }): Promise<RestoreDraftMaterialization | undefined>;
  completeRecoveryDraft(source: Pick<RestoreIntentRecord, 'pageId' | 'generation'>,
    target: RestoreGeneration, threadId: string, reviewed?: boolean): Promise<void>;
}

/** Only synthetic attachment stubs can share content destinations. A composer,
 * queued revision or uncertain send retains its own text and review context. */
function standaloneUpload(draft: ComposeDraftSource<StoredUploadSource>): StoredUploadSource | undefined {
  const upload = draft.uploads?.length === 1 ? draft.uploads[0] : undefined;
  return upload && !draft.unsentMessage && !draft.queuedUploadSend &&
    draft.threadId === historicalUploadDraft(upload).threadId ? upload : undefined;
}

interface Observation<Upload extends UploadSourceMetadata = StoredUploadSource> {
  generation: RestoreGeneration;
  recoveries: RestoreIntentRecord<Upload>[];
}

function selectHistoricalDrafts<Upload extends UploadSourceMetadata>(records: RestoreIntentRecord[],
  intent: PreservedComposeIntent<Upload>) {
  const previous = records.flatMap((r) => r.intent.drafts);
  const known = new Set(previous.map(historicalSourceId));
  const knownUploads = new Map(previous.flatMap((d) => d.uploads?.map((u) => [uploadSourceVersion(u), u] as const) ?? []));
  const knownSends = new Set(records.flatMap((r) => r.intent.unsentMessages?.map((u) => `send:${u.eventId}`) ?? []));
  const knownQueues = new Set(records.flatMap((r) => r.intent.queuedUploadSends?.map((q) => `queue:${queuedRestoreSourceId(q)}`) ?? []));
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
  return { drafts, knownUploads };
}

async function readHistoricalDrafts(observation: Observation, intent: PreservedComposeIntent<PendingUploadRecord | StoredUploadSource>,
  readBytes: (source: StoredUploadSource) => Promise<ArrayBuffer | undefined>): Promise<PreservedComposeIntent<PendingUploadRecord | StoredUploadSource>> {
  const { drafts, knownUploads } = selectHistoricalDrafts(observation.recoveries, intent);
  const payloads = new Map<string, ArrayBuffer>();
  const captured: ComposeDraftSource<PendingUploadRecord | StoredUploadSource>[] = [];
  for (const { uploads, ...draft } of drafts) {
    const attachments: (PendingUploadRecord | StoredUploadSource)[] | undefined = uploads && [];
    for (const upload of uploads ?? []) {
      if (!('byteKey' in upload)) {
        attachments!.push(upload);
        continue;
      }
      const known = knownUploads.get(uploadSourceVersion(upload));
      if (known) {
        attachments!.push(known);
        continue;
      }
      const { byteKey, ...metadata } = upload;
      const key = JSON.stringify(byteKey);
      const bytes = payloads.get(key) ?? await readBytes(upload);
      if (bytes === undefined) continue;
      payloads.set(key, bytes);
      attachments!.push({ ...metadata, bytes });
    }
    // A concurrent delete can remove the only content of an upload stub.
    if (draft.unsentMessage || draft.queuedUploadSend || attachments?.length) {
      captured.push({ ...draft, uploads: attachments });
    }
  }
  return { ...intent, drafts: captured };
}

/** Queue each lookup after the previous insert has succeeded so duplicate
 * payloads in the same transaction see each other. */
function retainDrafts(store: IDBObjectStore, drafts: ComposeDraftSource<PendingUploadRecord | StoredUploadSource>[],
  migrating: boolean, done: (drafts: ComposeDraftSource<StoredUploadSource>[]) => void): void {
  const retained = drafts.map(({ uploads, ...draft }): ComposeDraftSource<StoredUploadSource> =>
    uploads === undefined ? draft : { ...draft, uploads: [] });
  const attachments = drafts.flatMap((draft, index) => (draft.uploads ?? []).map(upload => ({ upload, index })));
  let cursor = 0;
  function next(): void {
    while (cursor < attachments.length) {
      const { upload, index } = attachments[cursor++];
      if ('byteKey' in upload) {
        retained[index].uploads!.push(upload);
      } else {
        retainUploadBytes(store, upload, migrating, (source) => {
          retained[index].uploads!.push(source);
          next();
        });
        return;
      }
    }
    done(retained);
  }
  next();
}

/** The restore journal's base name. The http-to-https hand-off carries it
 *  too (`public/outbox-handoff.js`). */
export const RESTORE_GENERATION_DATABASE = 'lucidos-restore-generation';

export function restoreGenerationDbName(workspaceId: string | null): string {
  return deviceDatabaseName(RESTORE_GENERATION_DATABASE, workspaceId);
}

export const RESTORE_OBSERVATIONS_STORE = 'observations';
const STORE = RESTORE_OBSERVATIONS_STORE;
const KEY = 'workspace';
const RESTORE_GENERATION_VERSION = 2;

function openRestoreGenerationDatabase(workspaceId: string | null) {
  return openDeviceDatabase(restoreGenerationDbName(workspaceId), (db, tx) => {
    const bytes = db.createObjectStore(UPLOAD_BYTES);
    if (!db.objectStoreNames.contains(STORE)) {
      db.createObjectStore(STORE);
      return;
    }
    const observations = tx.objectStore(STORE);
    const request = observations.get(KEY);
    request.onsuccess = () => {
      const previous = request.result as Observation<PendingUploadRecord> | undefined;
      if (!previous) return;
      retainDrafts(bytes, previous.recoveries.flatMap(record => record.intent.drafts), true, (drafts) => {
        let offset = 0;
        const recoveries = previous.recoveries.map(({ intent, ...record }) => {
          const start = offset;
          offset += intent.drafts.length;
          return { ...record, intent: { ...intent, drafts: drafts.slice(start, offset) } };
        });
        observations.put({ ...previous, recoveries } satisfies Observation, KEY);
      });
    };
  }, RESTORE_GENERATION_VERSION);
}

/** Both stores of one workspace's journal, each entry with its key. */
export interface RestoreJournal {
  observations: Array<[IDBValidKey, unknown]>;
  uploadBytes: Array<[IDBValidKey, unknown]>;
}

/** This origin's observation, with the carried recoveries it lacks added. The
 *  live observation keeps its generation: this page saw the server now. */
function mergeObservation(held: Observation, carried: Observation): Observation {
  const has = (r: RestoreIntentRecord) => held.recoveries.some((h) =>
    h.pageId === r.pageId && h.generation === r.generation);
  return { ...held, recoveries: [...held.recoveries, ...carried.recoveries.filter((r) => !has(r))] };
}

/** Write a journal the hand-off carried from the http origin. Upload bytes are
 *  added only where absent, and differing bytes under one key abort. The
 *  observation merges into one this origin already made, so a retry after a
 *  lost acknowledgement succeeds and nothing here is ever overwritten. */
export async function importRestoreJournal(workspaceId: string | null, journal: RestoreJournal): Promise<void> {
  const { transact } = openRestoreGenerationDatabase(workspaceId);
  let conflict = false;
  await transact([STORE, UPLOAD_BYTES], 'readwrite', (observations) => {
    const tx = observations.transaction;
    const bytes = tx.objectStore(UPLOAD_BYTES);
    for (const [key, value] of journal.uploadBytes) {
      const get = bytes.get(key);
      get.onsuccess = () => {
        if (get.result === undefined) bytes.put(value, key);
        else if (!sameUploadBytes((get.result as { bytes: ArrayBuffer }).bytes, (value as { bytes: ArrayBuffer }).bytes)) {
          conflict = true;
          tx.abort();
        }
      };
    }
    for (const [key, value] of journal.observations) {
      const get = observations.get(key);
      get.onsuccess = () => {
        const held = get.result as Observation | undefined;
        observations.put(held ? mergeObservation(held, value as Observation) : value, key);
      };
    }
  }).catch((err: unknown) => {
    if (conflict) throw new Error('the secure address already holds other upload bytes for a recovered draft');
    throw err;
  });
}

export function createRestoreGenerationStore(workspaceId: string | null, pageId: string):
RestoreIntentStore {
  const { transact } = openRestoreGenerationDatabase(workspaceId);
  const stores = [STORE, UPLOAD_BYTES];
  const read = () => transact<Observation>(STORE, 'readonly', (s) => s.get(KEY), BOUNDED);
  async function write(generation: RestoreGeneration, intent?: PreservedComposeIntent, expected?: GenerationExpectation): Promise<void> {
    let conflicted = false;
    // Read/modify/write in ONE transaction: tabs cannot erase each other's
    // preserved intent, and an observation cannot commit without its capture.
    // Bounded, a preserve included: an aborted capture is captured again on
    // the retry, and the backoff gives a large one longer each time.
    await transact(intent === undefined ? STORE : stores, 'readwrite', (s) => {
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
        if (intent === undefined || alreadyPreserved) {
          s.put({ generation, recoveries } satisfies Observation, KEY);
          return;
        }
        retainDrafts(s.transaction.objectStore(UPLOAD_BYTES), intent.drafts, false, (drafts) => {
          recoveries.push({ pageId, generation, intent: { ...intent, drafts } });
          s.put({ generation, recoveries } satisfies Observation, KEY);
        });
      };
    }, BOUNDED);
    if (conflicted) throw new Error('Restore generation changed in another tab');
  }
  return {
    readGeneration: async () => (await read())?.generation,
    readRecoveries: async () => (await read())?.recoveries ?? [],
    async materializeDraft({ uploads, ...draft }) {
      if (!uploads?.length) return uploads === undefined ? draft : { ...draft, uploads: [] };
      const requests = new Map<string, IDBRequest<{ bytes: ArrayBuffer } | undefined>>();
      await transact(UPLOAD_BYTES, 'readonly', (s) => {
        for (const { byteKey } of uploads) {
          const key = JSON.stringify(byteKey);
          if (!requests.has(key)) requests.set(key, readUploadPayload(s, byteKey));
        }
      }, BOUNDED);
      return { ...draft, uploads: uploads.map(({ byteKey, ...metadata }) => {
        const payload = requests.get(JSON.stringify(byteKey))?.result;
        if (!payload) throw new Error('Preserved restore upload bytes are missing');
        return { ...metadata, bytes: payload.bytes };
      }) };
    },
    observe: (generation, expected) => write(generation, undefined, expected),
    preserve: (generation, intent, expected) => write(generation, intent, expected),
    async journalHistorical(generation, intent: PreservedComposeIntent<PendingUploadRecord | StoredUploadSource>, batchId,
      readBytes?: (source: StoredUploadSource) => Promise<ArrayBuffer | undefined>) {
      let captured: PreservedComposeIntent<PendingUploadRecord | StoredUploadSource>;
      if (readBytes) {
        const observation = await read();
        if (!observation || observation.generation !== generation) throw new Error('Restore generation changed before historical capture');
        captured = await readHistoricalDrafts(observation, intent, readBytes);
      } else {
        captured = intent;
      }
      let accepted = false;
      await transact(stores, 'readwrite', (s) => {
        const request = s.get(KEY);
        request.onsuccess = () => {
          const observation = request.result as Observation | undefined;
          if (!observation || observation.generation !== generation) return;
          accepted = true;
          const { drafts } = selectHistoricalDrafts(observation.recoveries, captured);
          if (drafts.length === 0) return;
          retainDrafts(s.transaction.objectStore(UPLOAD_BYTES), drafts, false, (retained) => {
            observation.recoveries.push({ pageId: batchId, generation, intent: {
              drafts: retained, pendingSelection: {},
              unsentMessages: retained.flatMap((d) => d.unsentMessage ? [d.unsentMessage] : []),
              queuedUploadSends: retained.flatMap((d) => d.queuedUploadSend ? [d.queuedUploadSend] : []),
            } });
            s.put(observation, KEY);
          });
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
          const destinations = (draft: ComposeDraftSource<StoredUploadSource>) => sources
            .filter((s) => draft.unsentMessage
              ? s.draft.unsentMessage?.eventId === draft.unsentMessage.eventId
              : draft.queuedUploadSend
                ? s.draft.queuedUploadSend !== undefined &&
                  queuedRestoreSourceId(s.draft.queuedUploadSend) === queuedRestoreSourceId(draft.queuedUploadSend)
              : standaloneUpload(draft)
                ? standaloneUpload(s.draft) !== undefined &&
                  sameStoredUpload(standaloneUpload(draft)!, standaloneUpload(s.draft)!)
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
