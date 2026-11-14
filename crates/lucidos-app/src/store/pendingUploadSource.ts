/** Immutable archive identity. Adoption and landing retain separate versions;
 * review may consolidate their content, but must never erase the originals. */
import type { PendingUploadRecord } from './pendingUploadRecords';

export function uploadSourceVersion(record: PendingUploadRecord): string {
  return JSON.stringify([record.localId, record.threadId, record.ownerId, record.sourceGeneration ?? null,
    record.name, record.mime, record.size, record.createdAt, record.landedHash ?? null]);
}

/** Metadata revisions of one upload may share a review destination only when
 * the immutable file identity and every byte agree. Never trust landedHash as
 * proof of equality, or merge independently attached files with equal bytes. */
export function sameUploadContentLineage(a: PendingUploadRecord, b: PendingUploadRecord): boolean {
  if (a.localId !== b.localId || a.threadId !== b.threadId || a.name !== b.name ||
      a.mime !== b.mime || a.size !== b.size || a.createdAt !== b.createdAt) return false;
  if (!a.bytes || !b.bytes || a.bytes.byteLength !== a.size || b.bytes.byteLength !== b.size) return false;
  const left = new Uint8Array(a.bytes), right = new Uint8Array(b.bytes);
  return left.length === right.length && left.every((value, index) => value === right[index]);
}
