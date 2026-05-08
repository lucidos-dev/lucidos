import { computed, effect } from '@preact/signals';

import { blobPreviewUrl } from '../../api/client';
import { focusedThreadId } from '../../store/store';
import { composeDrafts, getDraft, patchDraft } from '../../store/composeDrafts';
import { updateCompose } from '../../store/actions/compose';
import { safeRevokeObjectUrl } from '../../utils/objectUrl';

/** A confirmed-attached image — the hash points at a blob already on the
 *  engine's disk under `data/blobs/`. The `previewUrl` is what
 *  `<img src=...>` consumes; the browser caches it forever (immutable).
 *  This struct carries no bytes — same shape over the wire as the draft
 *  that holds it. */
export interface AttachedImage {
  hash: string;
  previewUrl: string;
}

const EMPTY: AttachedImage[] = [];

/** Hash → in-memory `blob:` URL for images uploaded in this browser session.
 *  Populated by `attachImageToActiveDraft` after a successful upload — the
 *  same `blob:` URL that powered the pending preview is handed off here so
 *  the confirmed image keeps rendering from the in-memory File instead of
 *  swapping to the server preview URL. The HTTP swap looks fine on Chromium
 *  but on iOS Safari PWA the new <img> re-fetches over the network even
 *  when the URL was just preloaded — visible as a brief black flash. The
 *  blob URL is rock-solid (in-memory, no fetch), so keeping it in the
 *  preview eliminates the swap entirely. Falls back to the server preview
 *  URL for hashes that originate elsewhere (rehydrated drafts, cross-device
 *  sync). */
const sessionBlobUrls = new Map<string, string>();

/** Hashes sent this session — kept in `sessionBlobUrls` past the post-send
 *  draft clear so the message bubble renders in-memory bytes. iOS Safari PWA
 *  does not reliably serve the freshly-uploaded preview URL from cache,
 *  surfacing as a brief black flash. */
const sentSessionHashes = new Set<string>();

/** Hand off ownership of a `blob:` URL to the session map. First-write
 *  wins so re-uploading the same image (same hash) doesn't trample the URL
 *  the preview is already rendering — the duplicate is revoked here so the
 *  underlying File can be GC'd. */
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

/** GC: drop session URLs for hashes that left every draft AND were never
 *  sent. Fires whenever `composeDrafts` mutates. Iteration is bounded by
 *  the number of open drafts × attached images so the per-keystroke cost
 *  is microseconds. */
effect(() => {
  // `composeDrafts.value` is the dependency that re-fires this effect; read
  // it before the early-exit so signals tracking sees it on the empty-map
  // pass too (otherwise the effect would only re-fire after the FIRST
  // upload, and would not fire again if every URL was dropped between runs).
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

/** Caller MUST invoke before the draft clear so the GC effect on the next
 *  signal flush sees the sent flag — see `sentSessionHashes`. */
export function markHashesAsSent(hashes: readonly string[]): void {
  for (const h of hashes) sentSessionHashes.add(h);
}

/** Returns null for hashes that originate elsewhere (rehydrated drafts,
 *  cross-device sync, post-reload) — caller falls back to the server URL. */
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

/** Append a confirmed hash to the draft. Called from `attachImageToActiveDraft`
 *  on a successful blob upload — the bytes are already on disk; this just
 *  records the reference. Goes through `updateCompose` so the keystroke
 *  debounce + cross-device PUT fires once for the change. */
export function addAttachedImageHash(threadId: string, hash: string): void {
  const next = [...getDraft(threadId).image_hashes, hash];
  updateCompose(threadId, { image_hashes: next });
}

export function removeAttachedImage(threadId: string, index: number): void {
  const next = getDraft(threadId).image_hashes.filter((_, i) => i !== index);
  updateCompose(threadId, { image_hashes: next });
}

/** Local-only: drop a hash without touching the server. Used by retry paths
 *  so we can prune a stale reference without firing a PUT compose. */
export function pruneAttachedImageLocal(threadId: string, hash: string): void {
  const next = getDraft(threadId).image_hashes.filter((h) => h !== hash);
  patchDraft(threadId, { image_hashes: next });
}

export function _resetSessionBlobUrlsForTesting(): void {
  for (const url of sessionBlobUrls.values()) safeRevokeObjectUrl(url);
  sessionBlobUrls.clear();
  sentSessionHashes.clear();
}
