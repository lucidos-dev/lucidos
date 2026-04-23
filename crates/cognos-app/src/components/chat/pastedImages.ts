import { signal, computed } from '@preact/signals';
import { focusedThreadId, focusedDraftId } from '../../store/store';
import {
  draftImagesKey,
  loadDraftImagesRaw,
  saveDraftImagesRaw,
} from '../../utils/draftStorage';

export interface PastedImage {
  base64: string;
  mimeType: string;
}

const LEGACY_IMAGES_KEY = 'cognos-draft-images';
const EMPTY: PastedImage[] = [];

/** Resolve the draft id images should be scoped to. When a thread is focused
 *  the thread's id is used directly; otherwise the active compose draft id
 *  takes its place so multiple unsent drafts each keep their own images. */
function draftKey(threadId: string | null): string {
  return threadId ?? focusedDraftId.value;
}

/**
 * Exposed only so tests can read the stored value back. Production code
 * goes through the module's public API; the migration helper is the only
 * caller that needs to write under this key directly.
 */
export function pastedImagesStorageKey(threadId: string | null): string {
  return draftImagesKey(draftKey(threadId));
}

const pastedImagesByThread = signal<Map<string, PastedImage[]>>(new Map());

export const pastedImagesForCurrentThread = computed<PastedImage[]>(() => {
  // Reading focusedDraftId.value here also subscribes the computed to draft
  // focus changes, so the strip refreshes when the user switches drafts.
  const key = focusedThreadId.value ?? focusedDraftId.value;
  return pastedImagesByThread.value.get(key) ?? EMPTY;
});

export function getPastedImages(threadId: string | null): PastedImage[] {
  return pastedImagesByThread.value.get(draftKey(threadId)) ?? EMPTY;
}

function persistToStorage(threadId: string | null, images: PastedImage[]) {
  const id = draftKey(threadId);
  if (images.length === 0) {
    saveDraftImagesRaw(id, null);
    return;
  }
  try {
    saveDraftImagesRaw(id, JSON.stringify(images));
  } catch {
    // localStorage full or unavailable — images still work in-memory
  }
}

function setEntry(threadId: string | null, images: PastedImage[]) {
  const key = draftKey(threadId);
  const hadKey = pastedImagesByThread.value.has(key);
  if (images.length === 0 && !hadKey) return;

  const next = new Map(pastedImagesByThread.value);
  if (images.length === 0) {
    next.delete(key);
  } else {
    next.set(key, images);
  }
  pastedImagesByThread.value = next;
  persistToStorage(threadId, images);
}

export function addPastedImage(threadId: string | null, image: PastedImage) {
  setEntry(threadId, [...getPastedImages(threadId), image]);
}

export function removePastedImage(threadId: string | null, index: number) {
  setEntry(threadId, getPastedImages(threadId).filter((_, i) => i !== index));
}

export function clearPastedImages(threadId: string | null) {
  setEntry(threadId, []);
}

/**
 * Load persisted images for `threadId` into the in-memory map. Once an entry
 * exists, in-memory wins — avoids overwriting freshly-pasted images with a
 * stale localStorage snapshot written by another tab.
 */
export function hydratePastedImages(threadId: string | null): PastedImage[] {
  const key = draftKey(threadId);
  if (pastedImagesByThread.value.has(key)) {
    return pastedImagesByThread.value.get(key)!;
  }
  const raw = loadDraftImagesRaw(key);
  if (!raw) return EMPTY;
  try {
    const parsed = JSON.parse(raw) as PastedImage[];
    if (parsed.length === 0) return EMPTY;
    const next = new Map(pastedImagesByThread.value);
    next.set(key, parsed);
    pastedImagesByThread.value = next;
    return parsed;
  } catch {
    return EMPTY;
  }
}

/**
 * One-time migration of the legacy global `cognos-draft-images` key into
 * the focused thread's slot. Deletes the legacy key after migration.
 */
export function migrateLegacyPastedImages(threadId: string | null): void {
  const raw = localStorage.getItem(LEGACY_IMAGES_KEY);
  if (!raw) return;
  saveDraftImagesRaw(draftKey(threadId), raw);
  localStorage.removeItem(LEGACY_IMAGES_KEY);
}

export function resetPastedImagesForTests() {
  pastedImagesByThread.value = new Map();
}
