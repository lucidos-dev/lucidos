/** In-flight image uploads, scoped per-thread. Stays out of the persisted
 *  draft so a refresh never broadcasts a half-uploaded blob. */

import { signal } from '@preact/signals';

import { safeRevokeObjectUrl } from '../utils/objectUrl';

export interface PendingUpload {
  localId: string;
  threadId: string;
  /** Caller MUST `URL.revokeObjectURL` once the entry is removed. */
  previewUrl: string;
  mime: string;
  status: 'uploading' | 'failed';
  error?: string;
  /** Held so the retry path can re-upload without re-prompting. */
  file: File;
}

export const pendingUploads = signal<Map<string, PendingUpload[]>>(new Map());

const EMPTY: PendingUpload[] = [];

export function getPendingUploads(threadId: string | null | undefined): PendingUpload[] {
  if (!threadId) return EMPTY;
  return pendingUploads.value.get(threadId) ?? EMPTY;
}

export function addPendingUpload(entry: PendingUpload): void {
  const map = new Map(pendingUploads.value);
  const list = map.get(entry.threadId) ?? [];
  map.set(entry.threadId, [...list, entry]);
  pendingUploads.value = map;
}

export function patchPendingUpload(
  threadId: string,
  localId: string,
  patch: Partial<PendingUpload>,
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

/** Send button reads this to disable while a hash isn't in the draft yet. */
export function hasInFlightUploads(threadId: string | null | undefined): boolean {
  if (!threadId) return false;
  const list = pendingUploads.value.get(threadId);
  if (!list) return false;
  return list.some((u) => u.status === 'uploading');
}

/** True when an entry for `localId` is still present on `threadId`. Used by
 *  the upload pipeline to detect mid-flight cancellation — the user clicking
 *  the X removes the pending entry, so a returning POST whose entry vanished
 *  must not commit the resulting hash to the draft. */
export function hasPendingUpload(threadId: string, localId: string): boolean {
  return pendingUploads.value.get(threadId)?.some((u) => u.localId === localId) ?? false;
}

export function _resetPendingUploadsForTesting(): void {
  for (const list of pendingUploads.value.values()) {
    for (const entry of list) safeRevokeObjectUrl(entry.previewUrl);
  }
  pendingUploads.value = new Map();
}
