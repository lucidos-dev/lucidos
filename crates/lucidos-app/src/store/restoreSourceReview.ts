/** A lossless view of retained restore sources. Eligibility means creating a
 * review draft only; it never authorizes sending or deleting the original.
 * Content remains in the journal, so this view does not copy attachment bytes. */
import type { RestoreGeneration } from './restoreGeneration';
import type { PreservedComposeIntent, RestoreIntentRecord } from './restoreGenerationRecords';
import { historicalUploadDraft } from './restoreHistoricalSources';

type Ownership = 'independent' | 'authoritative' | 'captured-owner' | 'live' | 'unknown' | 'orphan';

export interface RestoreSourceReview {
  record: RestoreIntentRecord;
  source: RestoreIntentRecord['intent']['drafts'][number];
  pendingSelection: PreservedComposeIntent['pendingSelection'];
  kind: 'composer' | 'queued-send' | 'uncertain-send' | 'upload';
  /** Queue history includes settled revisions. Delivery cannot be inferred
   * from its presence or from an archived server's event log. */
  delivery: 'unknown' | 'not-a-send';
  ownership: Ownership;
  automatic: boolean;
  destinations: Array<{ generation: RestoreGeneration; threadId: string; completed: boolean; reviewed?: true }>;
  /** Shared attachment content is a review relationship, never permission to
   * merge queued revisions, composers or uncertain sends. */
  related: Array<{ pageId: string; generation: RestoreGeneration; sourceThreadId: string }>;
}

export function restoreSourceReview(records: RestoreIntentRecord[], liveOwners: ReadonlySet<string> | null): RestoreSourceReview[] {
  const capturedOwners = new Set(records.flatMap((r) => r.intent.captureOwnerId ? [r.intent.captureOwnerId] : []));
  // Build attachment relationships only when a review consumer requests them.
  let relationships: Map<RestoreSourceReview, RestoreSourceReview['related']> | undefined;
  const relatedFor = (entry: RestoreSourceReview) => {
    relationships ??= linkAttachments(entries);
    return relationships.get(entry)!;
  };
  const entries = records.flatMap((record) => record.intent.drafts.map((source): RestoreSourceReview => {
    const owner = source.composerSource?.ownerId;
    const ownership: Ownership = !owner ? 'independent'
      : owner === record.intent.captureOwnerId ? 'authoritative'
      : capturedOwners.has(owner) ? 'captured-owner'
      : liveOwners === null ? 'unknown'
      : liveOwners.has(owner) ? 'live' : 'orphan';
    const upload = source.uploads?.length === 1 ? source.uploads[0] : undefined;
    const kind = source.unsentMessage ? 'uncertain-send' : source.queuedUploadSend ? 'queued-send'
      : upload && source.threadId === historicalUploadDraft(upload).threadId ? 'upload' : 'composer';
    return { record, source, pendingSelection: record.intent.pendingSelection, kind,
      delivery: kind === 'queued-send' || kind === 'uncertain-send' ? 'unknown' : 'not-a-send',
      ownership, automatic: ownership === 'independent' || ownership === 'authoritative' || ownership === 'orphan',
      get related() { return relatedFor(this); },
      destinations: [
        ...(record.materializations?.flatMap((m) => m.drafts.filter((d) => d.sourceThreadId === source.threadId)
          .map((d) => ({ generation: m.generation, threadId: d.threadId, completed: m.completed.includes(d.threadId) }))) ?? []),
        ...(record.reviewedMaterializations?.flatMap((m) => m.drafts.filter((d) => d.sourceThreadId === source.threadId)
          .map((d) => ({ generation: m.generation, threadId: d.threadId, completed: m.completed.includes(d.threadId), reviewed: true as const }))) ?? []),
      ] };
  }));
  return entries;
}

function linkAttachments(entries: RestoreSourceReview[]): Map<RestoreSourceReview, RestoreSourceReview['related']> {
  const relationships = new Map(entries.map((entry) => [entry, [] as RestoreSourceReview['related']]));
  // The byte keys come from this journal, so they include the legacy variant.
  const buckets = new Map<string, RestoreSourceReview[]>();
  for (const entry of entries) {
    const candidates = new Set<RestoreSourceReview>();
    for (const upload of entry.source.uploads ?? []) {
      const key = JSON.stringify(upload.byteKey);
      const bucket = buckets.get(key) ?? [];
      for (const other of bucket) {
        if (other !== entry) candidates.add(other);
      }
      if (!bucket.includes(entry)) bucket.push(entry);
      buckets.set(key, bucket);
    }
    const reference = (e: RestoreSourceReview) => ({ pageId: e.record.pageId,
      generation: e.record.generation, sourceThreadId: e.source.threadId });
    for (const other of candidates) {
      relationships.get(entry)!.push(reference(other));
      relationships.get(other)!.push(reference(entry));
    }
  }
  return relationships;
}
