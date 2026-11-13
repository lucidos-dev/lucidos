/** Capture before restore admission releases any thread loading or cleanup.
 * The caller freezes editing for this operation through the durable journal
 * commit and reload. Reads drain each store's writes; failures keep admission
 * closed. No source is adopted, expired, deleted or interpreted as a send. */
import type { RestoreSourceOrigin } from '../utils/restoreSourceGeneration';
import { queuedRestoreSourceId } from '../utils/restoreAdmission';
import type { ComposeDraft } from './composeDrafts';
import type { ComposeSelectionOverride } from './composeSelections';
import type { PreservedComposeIntent } from './restoreGenerationRecords';
import type { PendingUploadRecord, QueuedUploadSendRecord, readPendingUploadStore } from './pendingUploadRecords';
import type { UnsentMessageRecord, readUnsentMessageStore } from './unsentMessageRecords';

export interface RestoreIntentCaptureSources {
  ownerId?: string;
  readUploads: typeof readPendingUploadStore;
  readUnsent: typeof readUnsentMessageStore;
  readComposers(): LiveRestoreIntent | Promise<LiveRestoreIntent>;
}

export interface LiveRestoreIntent {
  drafts: ReadonlyMap<string, ComposeDraft>;
  selections: ReadonlyMap<string, ComposeSelectionOverride>;
  pendingSelection: ComposeSelectionOverride;
  uploads?: PendingUploadRecord[];
  unsent?: UnsentMessageRecord[];
  queued?: QueuedUploadSendRecord[];
}

export async function captureRestoreIntent(sources: RestoreIntentCaptureSources): Promise<PreservedComposeIntent> {
  const [pending, storedUnsent] = await Promise.all([sources.readUploads(), sources.readUnsent()]);
  const live = await sources.readComposers();
  // The live value wins: a cap or failed write must not erase an attachment
  // or a newer send merely because it is absent or older in IndexedDB.
  const uploads = [...new Map([...pending.uploads, ...(live.uploads ?? [])].map((r) => [r.localId, r])).values()];
  const unsent = [...new Map([...storedUnsent, ...(live.unsent ?? [])].map((r) => [r.eventId, r])).values()];
  const queued = [...new Map([...pending.queuedUploadSendRecords, ...(live.queued ?? [])].map((r) => [JSON.stringify([r.ownerId, r.threadId]), r])).values()];
  const drafts: PreservedComposeIntent['drafts'] = [...live.drafts].map(([threadId, draft]) => ({
    threadId, draft,
    composerSource: sources.ownerId ? { ownerId: sources.ownerId, threadId } : undefined,
    selection: live.selections.get(threadId) ?? {},
  }));
  // A local id or content hash alone says nothing about which retained intent
  // owns the bytes. Match the same boundary used by historical capture.
  const sameOrigin = (upload: PendingUploadRecord, source: RestoreSourceOrigin & { ownerId: string; threadId: string }) =>
    upload.ownerId === source.ownerId && upload.threadId === source.threadId &&
    (upload.sourceGeneration ?? null) === (source.sourceGeneration ?? null);
  for (const upload of uploads) {
    const foreign = sources.ownerId !== undefined && upload.ownerId !== sources.ownerId;
    const sourceThreadId = foreign ? `foreign-composer:${JSON.stringify([upload.ownerId, upload.threadId])}` : upload.threadId;
    let destination = drafts.find((d) => d.threadId === sourceThreadId);
    if (!destination && queued.some((q) => sameOrigin(upload, q) && q.localIds.includes(upload.localId))) continue;
    // Landed bytes may outlive their composer solely for an unresolved send.
    // Retain them with that source, unless the composer explicitly reuses them.
    if (upload.landedHash !== undefined &&
      unsent.some((s) => sameOrigin(upload, s) && s.body.image_hashes?.includes(upload.landedHash!)) &&
      !destination?.draft.image_hashes.includes(upload.landedHash)) continue;
    if (!destination) {
      destination = { threadId: sourceThreadId,
        composerSource: sources.ownerId ? { ownerId: upload.ownerId, threadId: upload.threadId } : undefined,
        draft: { text: '', image_hashes: [], mode: null },
        selection: foreign ? {} : live.selections.get(upload.threadId) ?? {} };
      drafts.push(destination);
    }
    (destination.uploads ??= []).push(upload);
  }
  // A send and a later composer edit are distinct intent, even on the same
  // thread. Namespaced source keys remain stable across lost-response retries.
  for (const record of unsent) {
    drafts.push({
      threadId: `unsent:${record.eventId}`,
      unsentMessage: record,
      uploads: uploads.filter((u) => sameOrigin(u, record) && u.landedHash !== undefined && record.body.image_hashes?.includes(u.landedHash)),
      draft: { text: record.body.message, image_hashes: record.body.image_hashes ?? [],
        mode: record.body.use_coding_agent ? 'claude_code' : 'lucidos' },
      selection: { model: record.body.model, provider: record.body.provider,
        reasoningEffort: record.body.reasoning_effort, codingAgent: record.body.coding_agent },
    });
  }
  for (const record of queued) {
    const composer = record.ownerId === sources.ownerId || sources.ownerId === undefined
      ? drafts.find((d) => d.threadId === record.threadId) : undefined;
    const mode = record.intent.useCodingAgent ? 'claude_code' : 'lucidos';
    if (composer?.draft.text === record.text && (composer.draft.mode ?? 'lucidos') === mode) {
      composer.queuedUploadSend = record;
      continue;
    }
    const attachments = uploads.filter((u) => sameOrigin(u, record) && record.localIds.includes(u.localId));
    drafts.push({ threadId: `queued-upload:${queuedRestoreSourceId(record)}`,
      queuedUploadSend: record,
      draft: { text: record.text, image_hashes: [...new Set(attachments.flatMap((u) => u.landedHash ? [u.landedHash] : []))], mode },
      selection: composer?.selection ?? {},
      uploads: attachments,
    });
  }
  if (Object.keys(live.pendingSelection).length > 0) {
    drafts.push({ threadId: 'pending-selection', draft: { text: '', image_hashes: [], mode: null },
      selection: live.pendingSelection });
  }
  // Includes ArrayBuffers. Later live edits or upload completions cannot mutate
  // the value while IndexedDB is committing the observation and capture.
  return structuredClone({ captureOwnerId: sources.ownerId, drafts, pendingSelection: live.pendingSelection,
    unsentMessages: unsent, queuedUploadSends: queued });
}
