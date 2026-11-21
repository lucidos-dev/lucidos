/** The image hashes the engine reported stored on each thread, from its
 *  `ImageUploaded` events, as this page has seen them. The event stream writes
 *  it; the upload pipeline (`store/actions/imageUploads.ts`) lands a pending
 *  image whose hash appears here. State rather than an event, so an image
 *  hashed after its event arrived still lands.
 *
 *  Plan: `docs/plans/2026-10-04-image-upload-lands-on-its-event.md`. */

import { signal } from '@preact/signals';

export const landedImages = signal<ReadonlyMap<string, ReadonlySet<string>>>(new Map());

export function noteImageLanded(threadId: string, hash: string): void {
  const hashes = landedImages.peek().get(threadId);
  if (hashes?.has(hash)) return;
  const map = new Map(landedImages.peek());
  map.set(threadId, new Set(hashes).add(hash));
  landedImages.value = map;
}

export function _resetLandedImagesForTesting(): void {
  landedImages.value = new Map();
}
