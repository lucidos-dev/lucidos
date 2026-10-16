/** In-flight image uploads, scoped per-thread. Stays out of the engine's
 *  draft, so other devices never see a half-uploaded image. The pipeline that
 *  moves an entry between states is `store/actions/imageUploads.ts`. Each
 *  entry's bytes are also kept on this device, so a reload can bring it back
 *  (`store/pendingUploadRecords.ts`). */

import { signal } from '@preact/signals';

import { safeRevokeObjectUrl } from '../utils/objectUrl';

/** Where one attached image is on its way to the engine. Every state but
 *  `failed` is still in flight. The composer draws each one differently, so
 *  an upload is never a chip that merely looks finished. */
export type PendingUploadState =
  /** The draft's `POST /threads` has not settled, so there is nowhere to upload to yet. */
  | { kind: 'waiting-for-thread' }
  | { kind: 'uploading'; sentBytes: number; totalBytes: number }
  /** The browser took the whole body and the engine has not answered yet. The
   *  bytes may still be in transit (`UploadObserver.onBodySent`). */
  | { kind: 'finishing' }
  /** A transient failure, backing off before attempt `attempt`. */
  | { kind: 'retrying'; attempt: number; reason: string }
  /** A transient failure while the engine connection is down. Resumes on reconnect. */
  | { kind: 'offline'; reason: string }
  /** Terminal. `retryable` is false for a verdict no retry can change. */
  | { kind: 'failed'; reason: string; retryable: boolean };

export interface PendingUpload {
  localId: string;
  threadId: string;
  /** Caller MUST `URL.revokeObjectURL` once the entry is removed. */
  previewUrl: string;
  mime: string;
  state: PendingUploadState;
  /** Held so a retry can re-upload without re-prompting. */
  file: File;
  /** The SHA-256 the engine names these bytes by, once computed. Its
   *  `ImageUploaded` event lands the entry even if the upload's own answer is
   *  lost. Absent without Web Crypto. */
  contentHash?: string;
}

export const pendingUploads = signal<Map<string, PendingUpload[]>>(new Map());

const EMPTY: PendingUpload[] = [];

export function getPendingUploads(threadId: string | null | undefined): PendingUpload[] {
  if (!threadId) return EMPTY;
  return pendingUploads.value.get(threadId) ?? EMPTY;
}

export function getPendingUpload(threadId: string, localId: string): PendingUpload | undefined {
  return pendingUploads.value.get(threadId)?.find((u) => u.localId === localId);
}

export function addPendingUpload(entry: PendingUpload): void {
  const map = new Map(pendingUploads.value);
  const list = map.get(entry.threadId) ?? [];
  map.set(entry.threadId, [...list, entry]);
  pendingUploads.value = map;
}

export function setPendingUploadState(
  threadId: string,
  localId: string,
  state: PendingUploadState,
): void {
  patchPendingUpload(threadId, localId, { state });
}

export function setPendingUploadContentHash(threadId: string, localId: string, contentHash: string): void {
  patchPendingUpload(threadId, localId, { contentHash });
}

function patchPendingUpload(
  threadId: string,
  localId: string,
  patch: Partial<Pick<PendingUpload, 'state' | 'contentHash'>>,
): void {
  const list = pendingUploads.value.get(threadId);
  if (!list) return;
  const idx = list.findIndex((u) => u.localId === localId);
  if (idx === -1) return;
  const next = [...list];
  next[idx] = { ...next[idx], ...patch };
  const map = new Map(pendingUploads.value);
  map.set(threadId, next);
  pendingUploads.value = map;
}

/** Remove and revoke the object URL. */
export function removePendingUpload(threadId: string, localId: string): void {
  const list = pendingUploads.value.get(threadId);
  if (!list) return;
  const entry = list.find((u) => u.localId === localId);
  if (entry) safeRevokeObjectUrl(entry.previewUrl);
  writeWithoutEntry(threadId, localId, list);
}

/** Remove without revoking — caller has taken ownership of the object URL
 *  (e.g. handed it to `sessionBlobUrls` so the confirmed image keeps
 *  rendering from the same in-memory File). Revoking here would invalidate
 *  the URL and break the preview the moment it transitions to confirmed. */
export function detachPendingUpload(threadId: string, localId: string): void {
  const list = pendingUploads.value.get(threadId);
  if (!list) return;
  writeWithoutEntry(threadId, localId, list);
}

function writeWithoutEntry(threadId: string, localId: string, list: PendingUpload[]): void {
  const next = list.filter((u) => u.localId !== localId);
  const map = new Map(pendingUploads.value);
  if (next.length === 0) map.delete(threadId);
  else map.set(threadId, next);
  pendingUploads.value = map;
}

/** What a thread's pending uploads mean for a send:
 *  - `clear`: nothing pending, every attached image is a confirmed hash;
 *  - `in-flight`: wait, the hashes are on their way;
 *  - `failed`: an image did not make it, and sending now would leave it out.
 *  `failed` wins over `in-flight`: the user has to act on it either way. */
export type UploadsGate = 'clear' | 'in-flight' | 'failed';

export function uploadsGate(threadId: string | null | undefined): UploadsGate {
  const list = getPendingUploads(threadId);
  if (list.some((u) => u.state.kind === 'failed')) return 'failed';
  return list.length > 0 ? 'in-flight' : 'clear';
}

/** True while any image on this thread is still on its way. A failed one is
 *  not: it becomes a hash only if the user retries it. */
export function hasInFlightUploads(threadId: string | null | undefined): boolean {
  return getPendingUploads(threadId).some((u) => u.state.kind !== 'failed');
}

/** True when an entry for `localId` is still present on `threadId`. Used by
 *  the upload pipeline to detect mid-flight cancellation — the user clicking
 *  the X removes the pending entry, so a returning POST whose entry vanished
 *  must not commit the resulting hash to the draft. */
export function hasPendingUpload(threadId: string, localId: string): boolean {
  return getPendingUpload(threadId, localId) !== undefined;
}

export function _resetPendingUploadsForTesting(): void {
  for (const list of pendingUploads.value.values()) {
    for (const entry of list) safeRevokeObjectUrl(entry.previewUrl);
  }
  pendingUploads.value = new Map();
}
