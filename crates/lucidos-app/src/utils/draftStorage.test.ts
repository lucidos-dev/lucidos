import { describe, it, expect, beforeEach } from 'vitest';
import {
  draftTextKey,
  draftImagesKey,
  draftUpdatedKey,
  loadDraftText,
  saveDraftText,
  loadDraftImagesRaw,
  saveDraftImagesRaw,
  loadDraftUpdatedAt,
  saveDraftUpdatedAt,
  deleteDraft,
  scanDraftIds,
  draftHasContent,
  DRAFT_TEXT_PREFIX,
  DRAFT_IMAGES_PREFIX,
  DRAFT_UPDATED_PREFIX,
} from './draftStorage';

beforeEach(() => {
  localStorage.clear();
});

describe('draftStorage key helpers', () => {
  it('draftTextKey returns the prefixed text key', () => {
    expect(draftTextKey('abc')).toBe(`${DRAFT_TEXT_PREFIX}abc`);
  });
  it('draftImagesKey returns the prefixed images key', () => {
    expect(draftImagesKey('abc')).toBe(`${DRAFT_IMAGES_PREFIX}abc`);
  });
  it('draftUpdatedKey returns the prefixed updated key', () => {
    expect(draftUpdatedKey('abc')).toBe(`${DRAFT_UPDATED_PREFIX}abc`);
  });
});

describe('text storage', () => {
  it('saveDraftText then loadDraftText round-trips', () => {
    saveDraftText('id-1', 'hello');
    expect(loadDraftText('id-1')).toBe('hello');
  });

  it('loadDraftText returns empty string when nothing is stored', () => {
    expect(loadDraftText('missing')).toBe('');
  });

  it('saveDraftText with empty string deletes the key', () => {
    saveDraftText('id-1', 'hello');
    saveDraftText('id-1', '');
    expect(localStorage.getItem(draftTextKey('id-1'))).toBeNull();
  });
});

describe('images raw storage', () => {
  it('saveDraftImagesRaw then loadDraftImagesRaw round-trips a JSON string', () => {
    const json = JSON.stringify([{ base64: 'AAA', mimeType: 'image/png' }]);
    saveDraftImagesRaw('id-1', json);
    expect(loadDraftImagesRaw('id-1')).toBe(json);
  });

  it('saveDraftImagesRaw with null deletes the key', () => {
    saveDraftImagesRaw('id-1', '[]');
    saveDraftImagesRaw('id-1', null);
    expect(localStorage.getItem(draftImagesKey('id-1'))).toBeNull();
  });

  it('loadDraftImagesRaw returns null when nothing is stored', () => {
    expect(loadDraftImagesRaw('missing')).toBeNull();
  });
});

describe('updatedAt storage', () => {
  it('saveDraftUpdatedAt then loadDraftUpdatedAt round-trips', () => {
    saveDraftUpdatedAt('id-1', '2026-04-18T10:00:00.000Z');
    expect(loadDraftUpdatedAt('id-1')).toBe('2026-04-18T10:00:00.000Z');
  });

  it('saveDraftUpdatedAt with null deletes the key', () => {
    saveDraftUpdatedAt('id-1', '2026-04-18T10:00:00.000Z');
    saveDraftUpdatedAt('id-1', null);
    expect(localStorage.getItem(draftUpdatedKey('id-1'))).toBeNull();
  });

  it('loadDraftUpdatedAt returns null when nothing is stored', () => {
    expect(loadDraftUpdatedAt('missing')).toBeNull();
  });
});

describe('deleteDraft', () => {
  it('removes text, images, and updatedAt for the given id', () => {
    saveDraftText('id-1', 'hello');
    saveDraftImagesRaw('id-1', '[{}]');
    saveDraftUpdatedAt('id-1', '2026-04-18T10:00:00.000Z');

    deleteDraft('id-1');

    expect(localStorage.getItem(draftTextKey('id-1'))).toBeNull();
    expect(localStorage.getItem(draftImagesKey('id-1'))).toBeNull();
    expect(localStorage.getItem(draftUpdatedKey('id-1'))).toBeNull();
  });

  it('does not affect other drafts', () => {
    saveDraftText('id-1', 'first');
    saveDraftText('id-2', 'second');

    deleteDraft('id-1');

    expect(loadDraftText('id-1')).toBe('');
    expect(loadDraftText('id-2')).toBe('second');
  });
});

describe('scanDraftIds', () => {
  it('returns ids for drafts with text only', () => {
    saveDraftText('id-1', 'hello');
    expect(scanDraftIds().sort()).toEqual(['id-1']);
  });

  it('returns ids for drafts with images only (no text)', () => {
    saveDraftImagesRaw('id-img', '[{"base64":"AAA","mimeType":"image/png"}]');
    expect(scanDraftIds().sort()).toEqual(['id-img']);
  });

  it('returns the union of text-only, image-only, and combined drafts', () => {
    saveDraftText('text-only', 'hi');
    saveDraftImagesRaw('img-only', '[{}]');
    saveDraftText('both', 'x');
    saveDraftImagesRaw('both', '[{}]');
    expect(scanDraftIds().sort()).toEqual(['both', 'img-only', 'text-only']);
  });

  it('does not return ids for drafts with empty text and no images', () => {
    saveDraftText('id-1', '');
    expect(scanDraftIds()).toEqual([]);
  });

  it('ignores unrelated localStorage keys', () => {
    localStorage.setItem('lucidos-some-other-key', 'x');
    localStorage.setItem('lucidos-focused-thread', 'whatever');
    saveDraftText('id-1', 'hello');
    expect(scanDraftIds().sort()).toEqual(['id-1']);
  });
});

describe('draftHasContent', () => {
  it('returns true when text is present', () => {
    saveDraftText('id-1', 'hello');
    expect(draftHasContent('id-1')).toBe(true);
  });

  it('returns true when only images are present', () => {
    saveDraftImagesRaw('id-1', '[{"base64":"A","mimeType":"image/png"}]');
    expect(draftHasContent('id-1')).toBe(true);
  });

  it('returns false when neither text nor images are present', () => {
    expect(draftHasContent('id-1')).toBe(false);
  });

  it('returns false when images is an empty JSON array', () => {
    saveDraftImagesRaw('id-1', '[]');
    expect(draftHasContent('id-1')).toBe(false);
  });
});
