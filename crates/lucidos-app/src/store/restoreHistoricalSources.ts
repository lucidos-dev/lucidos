/** Late durable writes from old pages are journaled before ordinary startup.
 * This capture asserts no ownership of a live composer's current contents. */
import type { RestoreGeneration } from './restoreGeneration';
import type { PendingUploadRecord, QueuedUploadSendRecord } from './pendingUploadRecords';
import type { UnsentMessageRecord } from './unsentMessageRecords';
import type { PreservedComposeIntent } from './restoreGenerationRecords';
import { captureRestoreIntent } from './restoreIntentCapture';
import { uploadSourceVersion } from './pendingUploadSource';
import { queuedRestoreSourceId } from '../utils/restoreAdmission';

export async function captureHistoricalSources(generation: RestoreGeneration,
  pending: { uploads: PendingUploadRecord[]; queuedUploadSendRecords: QueuedUploadSendRecord[] },
  unsent: UnsentMessageRecord[]): Promise<PreservedComposeIntent> {
  const historical = (record: { sourceGeneration?: RestoreGeneration }) => (record.sourceGeneration ?? null) !== generation;
  const sends = unsent.filter(historical);
  const queued = pending.queuedUploadSendRecords.filter(historical);
  const sameOrigin = (u: PendingUploadRecord, source: { ownerId: string; threadId: string; sourceGeneration?: RestoreGeneration }) =>
    u.ownerId === source.ownerId && u.threadId === source.threadId &&
    (u.sourceGeneration ?? null) === (source.sourceGeneration ?? null);
  const uploads = pending.uploads.filter(historical);
  // Capture each immutable queue revision separately. Ordinary live capture
  // merges active slots, which would erase earlier retained queue revisions.
  const captures = await Promise.all([
    ...queued.map((q) => captureRestoreIntent({
      readUploads: async () => ({ uploads: [], queuedUploadSendRecords: [q] }), readUnsent: async () => [],
      readComposers: () => ({ drafts: new Map(), selections: new Map(), pendingSelection: {} }),
    })),
    captureRestoreIntent({
      readUploads: async () => ({ uploads: [], queuedUploadSendRecords: [] }), readUnsent: async () => sends,
      readComposers: () => ({ drafts: new Map(), selections: new Map(), pendingSelection: {} }),
    }),
  ]);
  const intent: PreservedComposeIntent = { drafts: captures.flatMap((c) => c.drafts), pendingSelection: {},
    queuedUploadSends: queued, unsentMessages: sends };
  const referenced = new Set<string>();
  for (const draft of intent.drafts) {
    const attachments = uploads.filter((u) => draft.queuedUploadSend
      ? sameOrigin(u, draft.queuedUploadSend) && draft.queuedUploadSend.localIds.includes(u.localId)
      : draft.unsentMessage && sameOrigin(u, draft.unsentMessage) &&
        u.landedHash !== undefined && draft.unsentMessage.body.image_hashes?.includes(u.landedHash));
    draft.uploads = attachments;
    for (const u of attachments) referenced.add(uploadSourceVersion(u));
  }
  for (const upload of uploads.filter((u) => !referenced.has(uploadSourceVersion(u)))) {
    intent.drafts.push(historicalUploadDraft(upload));
  }
  return intent;
}

/** Shared by the transactional journal and tests; excludes capture metadata. */
export function historicalSourceId(draft: PreservedComposeIntent['drafts'][number]): string {
  if (draft.unsentMessage) return `send:${draft.unsentMessage.eventId}`;
  if (draft.queuedUploadSend) return `queue:${queuedRestoreSourceId(draft.queuedUploadSend)}`;
  return `upload:${draft.uploads?.[0] ? uploadSourceVersion(draft.uploads[0]) : draft.threadId}`;
}

/** Each retained version needs its own destination key, including after an
 * earlier version's stub has already completed. These are never send requests. */
export function historicalUploadDraft(upload: PendingUploadRecord): PreservedComposeIntent['drafts'][number] {
  const threadId = `late-upload:${uploadSourceVersion(upload)}`;
  return { threadId, composerSource: { ownerId: upload.ownerId, threadId },
    draft: { text: '', image_hashes: [], mode: null }, selection: {}, uploads: [upload] };
}
