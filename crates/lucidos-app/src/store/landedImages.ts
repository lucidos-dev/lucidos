
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
