/**
 * Low-level localStorage I/O for unsent prompt drafts.
 *
 *   lucidos-draft:<id>          → raw textarea contents (string)
 *   lucidos-draft-images:<id>   → JSON-encoded PastedImage[] (string)
 *   lucidos-draft-updated:<id>  → ISO timestamp of the last edit (string)
 *
 * The image payload is owned by `components/chat/pastedImages.ts`; this
 * module only stores/loads the raw JSON it produces.
 */

export const DRAFT_TEXT_PREFIX = 'lucidos-draft:';
export const DRAFT_IMAGES_PREFIX = 'lucidos-draft-images:';
export const DRAFT_UPDATED_PREFIX = 'lucidos-draft-updated:';

/** Pointer storage keys identifying which thread / draft is currently in
 *  focus across reloads. Exported so every caller uses the same string. */
export const FOCUSED_THREAD_KEY = 'lucidos-focused-thread';
export const FOCUSED_DRAFT_KEY = 'lucidos-focused-draft';

export function draftTextKey(id: string): string {
  return DRAFT_TEXT_PREFIX + id;
}

export function draftImagesKey(id: string): string {
  return DRAFT_IMAGES_PREFIX + id;
}

export function draftUpdatedKey(id: string): string {
  return DRAFT_UPDATED_PREFIX + id;
}

export function loadDraftText(id: string): string {
  return localStorage.getItem(draftTextKey(id)) ?? '';
}

export function saveDraftText(id: string, text: string): void {
  if (text) {
    localStorage.setItem(draftTextKey(id), text);
  } else {
    localStorage.removeItem(draftTextKey(id));
  }
}

export function loadDraftImagesRaw(id: string): string | null {
  return localStorage.getItem(draftImagesKey(id));
}

export function saveDraftImagesRaw(id: string, raw: string | null): void {
  if (raw === null) {
    localStorage.removeItem(draftImagesKey(id));
  } else {
    localStorage.setItem(draftImagesKey(id), raw);
  }
}

export function loadDraftUpdatedAt(id: string): string | null {
  return localStorage.getItem(draftUpdatedKey(id));
}

export function saveDraftUpdatedAt(id: string, updatedAt: string | null): void {
  if (updatedAt === null) {
    localStorage.removeItem(draftUpdatedKey(id));
  } else {
    localStorage.setItem(draftUpdatedKey(id), updatedAt);
  }
}

/** Remove all storage entries (text, images, updatedAt) for a single draft. */
export function deleteDraft(id: string): void {
  localStorage.removeItem(draftTextKey(id));
  localStorage.removeItem(draftImagesKey(id));
  localStorage.removeItem(draftUpdatedKey(id));
}

/** True when the images payload is non-null AND parses to a non-empty array.
 *  Image-only drafts must remain visible so the user can find their pasted
 *  attachments even if they cleared the text. */
function hasNonEmptyImages(raw: string | null): boolean {
  if (!raw) return false;
  try {
    const parsed = JSON.parse(raw) as unknown;
    return Array.isArray(parsed) && parsed.length > 0;
  } catch {
    return false;
  }
}

export function draftHasContent(id: string): boolean {
  if (loadDraftText(id).length > 0) return true;
  return hasNonEmptyImages(loadDraftImagesRaw(id));
}

/** Find every draft ID with at least some content (text or images) in
 *  localStorage. Used on startup to populate the in-memory draft index. */
export function scanDraftIds(): string[] {
  const ids = new Set<string>();
  for (let i = 0; i < localStorage.length; i++) {
    const key = localStorage.key(i);
    if (!key) continue;
    if (key.startsWith(DRAFT_TEXT_PREFIX)) {
      const id = key.slice(DRAFT_TEXT_PREFIX.length);
      if ((localStorage.getItem(key) ?? '').length > 0) ids.add(id);
    } else if (key.startsWith(DRAFT_IMAGES_PREFIX)) {
      const id = key.slice(DRAFT_IMAGES_PREFIX.length);
      if (hasNonEmptyImages(localStorage.getItem(key))) ids.add(id);
    }
  }
  return [...ids];
}
