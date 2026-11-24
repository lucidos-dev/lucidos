import { computed, effect } from '@preact/signals';

import { blobPreviewUrl } from '../../api/client';
import { focusedThreadId } from '../../store/store';
import { composeDrafts, getDraft } from '../../store/composeDrafts';
import { updateCompose } from '../../store/actions/compose';
import { safeRevokeObjectUrl } from '../../utils/objectUrl';

export interface AttachedImage {
  hash: string;
  previewUrl: string;
}

const EMPTY: AttachedImage[] = [];

const sessionBlobUrls = new Map<string, string>();

const sentSessionHashes = new Set<string>();

export function rememberSessionBlobUrl(hash: string, blobUrlValue: string): void {
  if (sessionBlobUrls.has(hash)) {
    safeRevokeObjectUrl(blobUrlValue);
    return;
  }
  sessionBlobUrls.set(hash, blobUrlValue);
}

function dropSessionBlobUrl(hash: string): void {
  const url = sessionBlobUrls.get(hash);
  if (!url) return;
  safeRevokeObjectUrl(url);
  sessionBlobUrls.delete(hash);
}

effect(() => {
  const drafts = composeDrafts.value;
  if (sessionBlobUrls.size === 0) return;
  const referenced = new Set<string>();
  for (const draft of drafts.values()) {
    for (const h of draft.image_hashes) referenced.add(h);
  }
  for (const hash of [...sessionBlobUrls.keys()]) {
    if (!referenced.has(hash) && !sentSessionHashes.has(hash)) dropSessionBlobUrl(hash);
  }
});

export function markHashesAsSent(hashes: readonly string[]): void {
  for (const h of hashes) sentSessionHashes.add(h);
}

export function getSessionBlobUrlForHash(hash: string): string | null {
  return sessionBlobUrls.get(hash) ?? null;
}

export function getAttachedImages(threadId: string | null): AttachedImage[] {
  const wire = getDraft(threadId).image_hashes;
  if (wire.length === 0) return EMPTY;
  return wire.map((hash) => ({
    hash,
    previewUrl: getSessionBlobUrlForHash(hash) ?? blobPreviewUrl(hash),
  }));
}

export const attachedImagesForCurrentThread = computed<AttachedImage[]>(() =>
  getAttachedImages(focusedThreadId.value),
);

export function addAttachedImageHash(threadId: string, hash: string): boolean {
  const current = getDraft(threadId).image_hashes;
  if (current.includes(hash)) return false;
  updateCompose(threadId, { image_hashes: [...current, hash] });
  return true;
}

export function removeAttachedImage(threadId: string, index: number): void {
  const next = getDraft(threadId).image_hashes.filter((_, i) => i !== index);
  updateCompose(threadId, { image_hashes: next });
}

export function _resetSessionBlobUrlsForTesting(): void {
  for (const url of sessionBlobUrls.values()) safeRevokeObjectUrl(url);
  sessionBlobUrls.clear();
  sentSessionHashes.clear();
}
