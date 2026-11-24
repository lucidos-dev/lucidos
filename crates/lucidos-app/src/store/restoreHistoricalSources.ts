import type { RestoreGeneration } from './restoreGeneration';
import type { QueuedUploadSendRecord } from './pendingUploadRecords';
import type { UnsentMessageRecord } from './unsentMessageRecords';
import type { PreservedComposeIntent } from './restoreGenerationRecords';
import { captureRestoreIntent } from './restoreIntentCapture';
import { uploadSourceVersion, type UploadSourceMetadata } from './pendingUploadSource';
import { queuedRestoreSourceId } from '../utils/restoreAdmission';

export async function captureHistoricalSources<Upload extends UploadSourceMetadata>(generation: RestoreGeneration,
  pending: { uploads: Upload[]; queuedUploadSendRecords: QueuedUploadSendRecord[] },
  unsent: UnsentMessageRecord[]): Promise<PreservedComposeIntent<Upload>> {
  const historical = (record: { sourceGeneration?: RestoreGeneration }) => (record.sourceGeneration ?? null) !== generation;
  const sends = unsent.filter(historical);
  const queued = pending.queuedUploadSendRecords.filter(historical);
  const sameOrigin = (u: UploadSourceMetadata, source: { ownerId: string; threadId: string; sourceGeneration?: RestoreGeneration }) =>
    u.ownerId === source.ownerId && u.threadId === source.threadId &&
    (u.sourceGeneration ?? null) === (source.sourceGeneration ?? null);
  const uploads = pending.uploads.filter(historical);
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
  const intent: PreservedComposeIntent<Upload> = { drafts: captures.flatMap((c) => c.drafts.map((draft) => ({ ...draft, uploads: undefined }))), pendingSelection: {},
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

export function historicalSourceId(draft: PreservedComposeIntent<UploadSourceMetadata>['drafts'][number]): string {
  if (draft.unsentMessage) return `send:${draft.unsentMessage.eventId}`;
  if (draft.queuedUploadSend) return `queue:${queuedRestoreSourceId(draft.queuedUploadSend)}`;
  return `upload:${draft.uploads?.[0] ? uploadSourceVersion(draft.uploads[0]) : draft.threadId}`;
}

export function historicalUploadDraft<Upload extends UploadSourceMetadata>(upload: Upload): PreservedComposeIntent<Upload>['drafts'][number] {
  const threadId = `late-upload:${uploadSourceVersion(upload)}`;
  return { threadId, composerSource: { ownerId: upload.ownerId, threadId },
    draft: { text: '', image_hashes: [], mode: null }, selection: {}, uploads: [upload] };
}
