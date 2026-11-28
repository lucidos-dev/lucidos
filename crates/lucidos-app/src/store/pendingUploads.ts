
import { signal } from '@preact/signals';

import { safeRevokeObjectUrl } from '../utils/objectUrl';

export type PendingUploadState =
  | { kind: 'waiting-for-thread' }
  | { kind: 'uploading'; sentBytes: number; totalBytes: number }
  | { kind: 'finishing' }
  | { kind: 'retrying'; attempt: number; reason: string }
  | { kind: 'offline'; reason: string }
  | { kind: 'failed'; reason: string; retryable: boolean };

export interface PendingUpload {
  localId: string;
  threadId: string;
  previewUrl: string;
  mime: string;
  state: PendingUploadState;
  file: File;
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

export function removePendingUpload(threadId: string, localId: string): void {
  const list = pendingUploads.value.get(threadId);
  if (!list) return;
  const entry = list.find((u) => u.localId === localId);
  if (entry) safeRevokeObjectUrl(entry.previewUrl);
  writeWithoutEntry(threadId, localId, list);
}

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

export type UploadsGate = 'clear' | 'in-flight' | 'failed';

export function uploadsGate(threadId: string | null | undefined): UploadsGate {
  const list = getPendingUploads(threadId);
  if (list.some((u) => u.state.kind === 'failed')) return 'failed';
  return list.length > 0 ? 'in-flight' : 'clear';
}

export function hasInFlightUploads(threadId: string | null | undefined): boolean {
  return getPendingUploads(threadId).some((u) => u.state.kind !== 'failed');
}

export function hasPendingUpload(threadId: string, localId: string): boolean {
  return getPendingUpload(threadId, localId) !== undefined;
}

export function _resetPendingUploadsForTesting(): void {
  for (const list of pendingUploads.value.values()) {
    for (const entry of list) safeRevokeObjectUrl(entry.previewUrl);
  }
  pendingUploads.value = new Map();
}
