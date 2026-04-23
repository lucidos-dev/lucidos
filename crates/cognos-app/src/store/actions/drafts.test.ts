import { describe, it, expect, beforeEach } from 'vitest';
import { drafts, focusedDraftId, focusedThreadId, newDraftId } from '../store';
import {
  createComposeDraft,
  focusDraft,
  discardDraft,
  syncDraftEntry,
  promoteDraftToThread,
} from './drafts';
import {
  saveDraftText,
  saveDraftImagesRaw,
  saveDraftUpdatedAt,
  loadDraftText,
  loadDraftImagesRaw,
  loadDraftUpdatedAt,
} from '../../utils/draftStorage';
import { DRAFT_FALLBACK_TITLE } from '../../utils/draftTitle';

beforeEach(() => {
  localStorage.clear();
  drafts.value = new Map();
  focusedThreadId.value = null;
  focusedDraftId.value = newDraftId();
});

describe('createComposeDraft', () => {
  it('returns a fresh draft id and focuses it', () => {
    const id = createComposeDraft();
    expect(id).toMatch(/^draft-/);
    expect(focusedDraftId.value).toBe(id);
    expect(focusedThreadId.value).toBeNull();
    expect(localStorage.getItem('cognos-focused-draft')).toBe(id);
  });

  it('always returns a different id from the previously focused draft', () => {
    const first = focusedDraftId.value;
    const second = createComposeDraft();
    expect(second).not.toBe(first);
  });

  it('does not create a draft entry until the user types', () => {
    createComposeDraft();
    expect(drafts.value.size).toBe(0);
  });

  it('clears any previously focused thread', () => {
    focusedThreadId.value = 'thread-x';
    localStorage.setItem('cognos-focused-thread', 'thread-x');
    createComposeDraft();
    expect(focusedThreadId.value).toBeNull();
    expect(localStorage.getItem('cognos-focused-thread')).toBeNull();
  });
});

describe('focusDraft', () => {
  it('sets focusedDraftId and clears focusedThreadId', () => {
    focusedThreadId.value = 'thread-x';
    localStorage.setItem('cognos-focused-thread', 'thread-x');

    focusDraft('draft-abc');

    expect(focusedDraftId.value).toBe('draft-abc');
    expect(focusedThreadId.value).toBeNull();
    expect(localStorage.getItem('cognos-focused-draft')).toBe('draft-abc');
    expect(localStorage.getItem('cognos-focused-thread')).toBeNull();
  });
});

describe('discardDraft', () => {
  it('removes text, images, updatedAt, and the map entry', () => {
    saveDraftText('id-1', 'hello');
    saveDraftImagesRaw('id-1', '[{"base64":"AAA","mimeType":"image/png"}]');
    saveDraftUpdatedAt('id-1', '2026-04-18T10:00:00.000Z');
    drafts.value = new Map([['id-1', { title: 'hello', updatedAt: '2026-04-18T10:00:00.000Z' }]]);

    discardDraft('id-1');

    expect(loadDraftText('id-1')).toBe('');
    expect(loadDraftImagesRaw('id-1')).toBeNull();
    expect(loadDraftUpdatedAt('id-1')).toBeNull();
    expect(drafts.value.has('id-1')).toBe(false);
  });

  it('replaces focusedDraftId with a fresh id when discarding the focused draft', () => {
    focusedDraftId.value = 'id-1';
    saveDraftText('id-1', 'hello');
    drafts.value = new Map([['id-1', { title: 'hello', updatedAt: '2026-04-18T10:00:00.000Z' }]]);

    discardDraft('id-1');

    expect(focusedDraftId.value).not.toBe('id-1');
    expect(focusedDraftId.value).toMatch(/^draft-/);
    expect(localStorage.getItem('cognos-focused-draft')).toBe(focusedDraftId.value);
  });

  it('leaves focusedDraftId untouched when discarding a different draft', () => {
    focusedDraftId.value = 'focused-id';
    drafts.value = new Map([
      ['focused-id', { title: 'focused', updatedAt: '' }],
      ['other-id', { title: 'other', updatedAt: '' }],
    ]);

    discardDraft('other-id');

    expect(focusedDraftId.value).toBe('focused-id');
    expect(drafts.value.has('focused-id')).toBe(true);
    expect(drafts.value.has('other-id')).toBe(false);
  });
});

describe('syncDraftEntry', () => {
  it('creates the map entry when text is added', () => {
    saveDraftText('id-1', 'hello world');
    syncDraftEntry('id-1');

    const meta = drafts.value.get('id-1');
    expect(meta).toBeDefined();
    expect(meta!.title).toBe('hello world');
    expect(meta!.updatedAt).not.toBe('');
    // updatedAt is also persisted so the sort order survives reload
    expect(loadDraftUpdatedAt('id-1')).toBe(meta!.updatedAt);
  });

  it('creates the map entry when images are added (no text)', () => {
    saveDraftImagesRaw('id-1', '[{"base64":"A","mimeType":"image/png"}]');
    syncDraftEntry('id-1');

    const meta = drafts.value.get('id-1');
    expect(meta).toBeDefined();
    expect(meta!.title).toBe(DRAFT_FALLBACK_TITLE);
  });

  it('removes the map entry and clears updatedAt when content is gone', () => {
    saveDraftText('id-1', 'hello');
    syncDraftEntry('id-1');
    expect(drafts.value.has('id-1')).toBe(true);

    saveDraftText('id-1', '');
    syncDraftEntry('id-1');

    expect(drafts.value.has('id-1')).toBe(false);
    expect(loadDraftUpdatedAt('id-1')).toBeNull();
  });

  it('refreshes the title when text changes', () => {
    saveDraftText('id-1', 'first version');
    syncDraftEntry('id-1');
    expect(drafts.value.get('id-1')!.title).toBe('first version');

    saveDraftText('id-1', 'second version');
    syncDraftEntry('id-1');
    expect(drafts.value.get('id-1')!.title).toBe('second version');
  });

  it('keeps the entry when text is empty but images are still attached', () => {
    saveDraftText('id-1', 'hello');
    saveDraftImagesRaw('id-1', '[{"base64":"A","mimeType":"image/png"}]');
    syncDraftEntry('id-1');
    expect(drafts.value.get('id-1')!.title).toBe('hello');

    saveDraftText('id-1', '');
    syncDraftEntry('id-1');
    expect(drafts.value.has('id-1')).toBe(true);
    expect(drafts.value.get('id-1')!.title).toBe(DRAFT_FALLBACK_TITLE);
  });
});

describe('promoteDraftToThread', () => {
  it('removes the draft entry and storage when sent', () => {
    saveDraftText('draft-1', 'message');
    saveDraftImagesRaw('draft-1', '[{}]');
    saveDraftUpdatedAt('draft-1', '2026-04-18T10:00:00.000Z');
    drafts.value = new Map([['draft-1', { title: 'message', updatedAt: '2026-04-18T10:00:00.000Z' }]]);

    promoteDraftToThread('draft-1');

    expect(loadDraftText('draft-1')).toBe('');
    expect(loadDraftImagesRaw('draft-1')).toBeNull();
    expect(loadDraftUpdatedAt('draft-1')).toBeNull();
    expect(drafts.value.has('draft-1')).toBe(false);
  });

  it('leaves the draft id off the focused-draft pointer when it was the focused one', () => {
    focusedDraftId.value = 'draft-1';
    localStorage.setItem('cognos-focused-draft', 'draft-1');
    saveDraftText('draft-1', 'message');
    drafts.value = new Map([['draft-1', { title: 'message', updatedAt: '' }]]);

    promoteDraftToThread('draft-1');

    // A fresh draft id is assigned so future composes start clean
    expect(focusedDraftId.value).not.toBe('draft-1');
    expect(focusedDraftId.value).toMatch(/^draft-/);
    expect(localStorage.getItem('cognos-focused-draft')).toBe(focusedDraftId.value);
  });
});
