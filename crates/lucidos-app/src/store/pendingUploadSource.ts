/** Immutable archive identity. Adoption and landing retain separate versions. */
import type { PendingUploadRecord } from './pendingUploadRecords';

export type UploadSourceMetadata = Omit<PendingUploadRecord, 'bytes'>;
export type UploadByteKey = [identity: string, variant: number];
export type StoredUploadSource = UploadSourceMetadata & { byteKey: UploadByteKey };

export function uploadSourceVersion(record: UploadSourceMetadata): string {
  return JSON.stringify([record.localId, record.threadId, record.ownerId, record.sourceGeneration ?? null,
    record.name, record.mime, record.size, record.createdAt, record.landedHash ?? null]);
}

export function uploadFileIdentity(record: UploadSourceMetadata): string {
  return JSON.stringify([record.localId, record.threadId, record.name, record.mime, record.size, record.createdAt]);
}

export function sameUploadBytes(a: ArrayBuffer, b: ArrayBuffer): boolean {
  const left = new Uint8Array(a), right = new Uint8Array(b);
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

/** Distinct payloads can share a lineage only if they came from a legacy store. */
export function sameStoredUpload(a: StoredUploadSource, b: StoredUploadSource): boolean {
  return a.byteKey[0] === b.byteKey[0] && a.byteKey[1] === b.byteKey[1];
}
