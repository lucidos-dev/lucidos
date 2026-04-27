import { drafts, focusedDraftId, focusedThreadId, newDraftId, type DraftMeta } from '../store';
import {
  deleteDraft,
  draftHasContent,
  loadDraftText,
  saveDraftUpdatedAt,
  FOCUSED_DRAFT_KEY,
  FOCUSED_THREAD_KEY,
} from '../../utils/draftStorage';
import { draftTitle } from '../../utils/draftTitle';
import { pushThreadNavState } from './thread-navigation';

function setFocusedDraft(id: string): void {
  focusedDraftId.value = id;
  localStorage.setItem(FOCUSED_DRAFT_KEY, id);
}

function clearFocusedThread(): void {
  focusedThreadId.value = null;
  localStorage.removeItem(FOCUSED_THREAD_KEY);
}

function setDraftEntry(id: string, meta: DraftMeta | null): void {
  const next = new Map(drafts.value);
  if (meta) next.set(id, meta);
  else next.delete(id);
  drafts.value = next;
}

export function createComposeDraft(): string {
  const id = newDraftId();
  setFocusedDraft(id);
  clearFocusedThread();
  pushThreadNavState({ type: 'draft', id });
  return id;
}

export function focusDraft(id: string): void {
  setFocusedDraft(id);
  clearFocusedThread();
  pushThreadNavState({ type: 'draft', id });
}

export function discardDraft(id: string): void {
  deleteDraft(id);
  setDraftEntry(id, null);
  if (focusedDraftId.value === id) {
    setFocusedDraft(newDraftId());
  }
}

/** Reconcile the in-memory drafts map with what's on disk for a single id.
 *  Bumps updatedAt so the drawer's sort order reflects the latest edit. */
export function syncDraftEntry(id: string): void {
  if (!draftHasContent(id)) {
    saveDraftUpdatedAt(id, null);
    setDraftEntry(id, null);
    return;
  }
  const updatedAt = new Date().toISOString();
  saveDraftUpdatedAt(id, updatedAt);
  setDraftEntry(id, {
    title: draftTitle(loadDraftText(id)),
    updatedAt,
  });
}

/** Drop the storage and meta entry for a draft that just became a real
 *  thread, and assign a fresh id to focusedDraftId so the next compose
 *  starts blank. */
export function promoteDraftToThread(draftId: string): void {
  deleteDraft(draftId);
  setDraftEntry(draftId, null);
  if (focusedDraftId.value === draftId) {
    setFocusedDraft(newDraftId());
  }
}
