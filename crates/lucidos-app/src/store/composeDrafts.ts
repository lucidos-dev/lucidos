/** Per-thread compose drafts (text, image hashes, mode), kept OUT of
 *  `threadMap` so per-keystroke writes don't re-render every component
 *  subscribed to threadMap. Cross-device sync goes through `compose.ts`
 *  (debounced PUT + SSE-driven `applyRemoteCompose`); both write here
 *  instead of into `ThreadMeta`. */

import { signal } from '@preact/signals';
import type { ComposeChannelMode } from './thread-events';

export interface ComposeDraft {
  text: string;
  /** Sha256 hashes of blobs uploaded via `POST /threads/:id/blobs`. */
  image_hashes: string[];
  /** Mutable channel pick while state='composing'. Locked once the thread
   *  goes active — readers should fall back to thread.meta.channel by then. */
  mode: ComposeChannelMode;
}

/** Stable reference returned by `getDraft` when no entry exists, so renders
 *  that read-then-iterate can't accidentally write to the shared default. */
export const EMPTY_DRAFT: ComposeDraft = Object.freeze({
  text: '',
  image_hashes: [],
  mode: null,
}) as ComposeDraft;

export const composeDrafts = signal<Map<string, ComposeDraft>>(new Map());

export function getDraft(threadId: string | null | undefined): ComposeDraft {
  if (!threadId) return EMPTY_DRAFT;
  return composeDrafts.value.get(threadId) ?? EMPTY_DRAFT;
}

/** True when the draft has neither typed text nor attached images. Mode is
 *  ignored — a fresh composing thread has a mode pick but no content yet. */
export function draftIsEmpty(draft: ComposeDraft): boolean {
  return draft.text.trim().length === 0 && draft.image_hashes.length === 0;
}

/** Patch a draft. Undefined fields preserve the prior value, so callers can
 *  send `{ text }` without resetting images/mode. */
export function patchDraft(threadId: string, patch: Partial<ComposeDraft>): void {
  const prev = composeDrafts.value.get(threadId) ?? EMPTY_DRAFT;
  const next: ComposeDraft = {
    text: patch.text ?? prev.text,
    image_hashes: patch.image_hashes ?? prev.image_hashes,
    mode: patch.mode !== undefined ? patch.mode : prev.mode,
  };
  const map = new Map(composeDrafts.value);
  map.set(threadId, next);
  composeDrafts.value = map;
}

/** Replace a draft wholesale — every field is authoritative; unset fields
 *  clear, not preserve (the difference vs `patchDraft`). */
export function setDraft(threadId: string, draft: ComposeDraft): void {
  const map = new Map(composeDrafts.value);
  map.set(threadId, draft);
  composeDrafts.value = map;
}

/** Drop a thread's draft entirely; subsequent `getDraft` returns EMPTY. */
export function clearDraft(threadId: string): void {
  if (!composeDrafts.value.has(threadId)) return;
  const map = new Map(composeDrafts.value);
  map.delete(threadId);
  composeDrafts.value = map;
}

/** Apply many draft writes as one signal update. Used by `loadAllThreads`
 *  to hydrate N rows without firing N re-renders (one Map clone, one signal
 *  write). `null` value means clear the entry. */
export function applyDraftBatch(updates: ReadonlyMap<string, ComposeDraft | null>): void {
  if (updates.size === 0) return;
  const map = new Map(composeDrafts.value);
  for (const [id, draft] of updates) {
    if (draft === null) map.delete(id);
    else map.set(id, draft);
  }
  composeDrafts.value = map;
}

export function _resetComposeDraftsForTesting(): void {
  composeDrafts.value = new Map();
}
