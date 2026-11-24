
import { signal } from '@preact/signals';
import type { ComposeChannelMode } from './thread-events';

export interface ComposeDraft {
  text: string;
  image_hashes: string[];
  mode: ComposeChannelMode;
}

const EMPTY_DRAFT: ComposeDraft = Object.freeze({
  text: '',
  image_hashes: [],
  mode: null,
}) as ComposeDraft;

export const composeDrafts = signal<Map<string, ComposeDraft>>(new Map());

export const draftPresentThreadIds = signal<ReadonlySet<string>>(new Set());

export function getDraft(threadId: string | null | undefined): ComposeDraft {
  if (!threadId) return EMPTY_DRAFT;
  return composeDrafts.value.get(threadId) ?? EMPTY_DRAFT;
}

export function draftIsEmpty(draft: ComposeDraft): boolean {
  return draft.text.trim().length === 0 && draft.image_hashes.length === 0;
}

function isPresent(draft: ComposeDraft | undefined): boolean {
  return draft !== undefined && !draftIsEmpty(draft);
}

function syncPresence(threadId: string, prev: ComposeDraft | undefined, next: ComposeDraft | undefined): void {
  const wasPresent = isPresent(prev);
  const nowPresent = isPresent(next);
  if (wasPresent === nowPresent) return;
  const set = new Set(draftPresentThreadIds.value);
  if (nowPresent) set.add(threadId);
  else set.delete(threadId);
  draftPresentThreadIds.value = set;
}

export function patchDraft(threadId: string, patch: Partial<ComposeDraft>): void {
  const prev = composeDrafts.value.get(threadId);
  const base = prev ?? EMPTY_DRAFT;
  const next: ComposeDraft = {
    text: patch.text ?? base.text,
    image_hashes: patch.image_hashes ?? base.image_hashes,
    mode: patch.mode !== undefined ? patch.mode : base.mode,
  };
  const map = new Map(composeDrafts.value);
  map.set(threadId, next);
  composeDrafts.value = map;
  syncPresence(threadId, prev, next);
}

export function setDraft(threadId: string, draft: ComposeDraft): void {
  const prev = composeDrafts.value.get(threadId);
  const map = new Map(composeDrafts.value);
  map.set(threadId, draft);
  composeDrafts.value = map;
  syncPresence(threadId, prev, draft);
}

export function clearDraft(threadId: string): void {
  const prev = composeDrafts.value.get(threadId);
  if (!prev) return;
  const map = new Map(composeDrafts.value);
  map.delete(threadId);
  composeDrafts.value = map;
  syncPresence(threadId, prev, undefined);
}

export function applyDraftBatch(updates: ReadonlyMap<string, ComposeDraft | null>): void {
  if (updates.size === 0) return;
  const map = new Map(composeDrafts.value);
  const presence = new Set(draftPresentThreadIds.value);
  let presenceChanged = false;
  for (const [id, draft] of updates) {
    const wasPresent = presence.has(id);
    if (draft === null) {
      map.delete(id);
      if (wasPresent) { presence.delete(id); presenceChanged = true; }
    } else {
      map.set(id, draft);
      const nowPresent = !draftIsEmpty(draft);
      if (nowPresent && !wasPresent) { presence.add(id); presenceChanged = true; }
      else if (!nowPresent && wasPresent) { presence.delete(id); presenceChanged = true; }
    }
  }
  composeDrafts.value = map;
  if (presenceChanged) draftPresentThreadIds.value = presence;
}

export function _resetComposeDraftsForTesting(): void {
  composeDrafts.value = new Map();
  draftPresentThreadIds.value = new Set();
}
