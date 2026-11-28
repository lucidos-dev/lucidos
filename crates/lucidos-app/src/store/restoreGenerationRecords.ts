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
  captureOwnerId?: string;
  drafts: Array<{
    threadId: string;
    composerSource?: { ownerId: string; threadId: string };
    draft: ComposeDraft;
    selection: ComposeSelectionOverride;
    uploads?: Upload[];
    unsentMessage?: UnsentMessageRecord;
    queuedUploadSend?: QueuedUploadSendRecord;
  }>;
  pendingSelection: ComposeSelectionOverride;
  unsentMessages?: UnsentMessageRecord[];
  queuedUploadSends?: QueuedUploadSendRecord[];
}

type ComposeDraftSource<Upload extends UploadSourceMetadata> = PreservedComposeIntent<Upload>['drafts'][number];

export interface RestoreIntentRecord<Upload extends UploadSourceMetadata = StoredUploadSource> {
  pageId: string;
  generation: RestoreGeneration;
  intent: PreservedComposeIntent<Upload>;
  materializations?: RestoreDraftMaterialization[];
  reviewedMaterializations?: RestoreDraftMaterialization[];
}

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
    if (draft.unsentMessage || draft.queuedUploadSend || attachments?.length) {
      captured.push({ ...draft, uploads: attachments });
    }
  }
  return { ...intent, drafts: captured };
}

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

export interface RestoreJournal {
  observations: Array<[IDBValidKey, unknown]>;
  uploadBytes: Array<[IDBValidKey, unknown]>;
}

function mergeObservation(held: Observation, carried: Observation): Observation {
  const has = (r: RestoreIntentRecord) => held.recoveries.some((h) =>
    h.pageId === r.pageId && h.generation === r.generation);
  return { ...held, recoveries: [...held.recoveries, ...carried.recoveries.filter((r) => !has(r))] };
}

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
    await transact(intent === undefined ? STORE : stores, 'readwrite', (s) => {
      const request = s.get(KEY);
      request.onsuccess = () => {
        const previous = request.result as Observation | undefined;
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
            result = { ...materialization, drafts: [destination] };
            s.put(observation, KEY);
            return;
          }
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
