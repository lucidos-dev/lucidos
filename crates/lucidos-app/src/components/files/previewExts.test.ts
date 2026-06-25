import { describe, it, expect } from 'vitest';
import { isEditableDataFile, previewMediaKind, RENDERABLE_EXTS, REPO_RENDERABLE_EXTS } from './previewExts';

describe('REPO_RENDERABLE_EXTS', () => {
  // Regression: the repo file/diff preview rendered .html into a live srcDoc
  // iframe, so toggling to the whole-file/rendered view on an app-shell HTML
  // (crates/lucidos-app/index.html) showed its inlined boot splash ("Opening
  // your workspace…") instead of the file. Repo HTML is source under review.
  it('excludes html and htm (repo HTML shows as source, not a live render)', () => {
    expect(REPO_RENDERABLE_EXTS).not.toContain('html');
    expect(REPO_RENDERABLE_EXTS).not.toContain('htm');
  });

  it('keeps the self-contained rendered types (md, csv, svg, slides)', () => {
    for (const ext of ['md', 'csv', 'svg', 'slides']) {
      expect(REPO_RENDERABLE_EXTS).toContain(ext);
    }
  });

  it('is exactly RENDERABLE_EXTS minus html/htm (artifact viewer still renders HTML)', () => {
    expect(RENDERABLE_EXTS).toContain('html');
    expect(REPO_RENDERABLE_EXTS).toEqual(RENDERABLE_EXTS.filter(e => e !== 'html' && e !== 'htm'));
  });
});

describe('previewMediaKind', () => {
  // Regression: the repo file viewer (RepoFileContent) fetched EVERY file as
  // text and rendered it line-numbered, so a PNG icon showed as raw bytes. It
  // must classify binary images as 'image' so they divert to a URL-pointed
  // <img> instead of the text path.
  it('classifies image extensions as image', () => {
    for (const ext of ['png', 'jpg', 'jpeg', 'gif', 'webp', 'ico', 'bmp']) {
      expect(previewMediaKind(ext)).toBe('image');
    }
  });

  it('classifies video and audio extensions', () => {
    expect(previewMediaKind('mp4')).toBe('video');
    expect(previewMediaKind('mov')).toBe('video');
    expect(previewMediaKind('mp3')).toBe('audio');
    expect(previewMediaKind('ogg')).toBe('audio');
  });

  it('classifies pdf', () => {
    expect(previewMediaKind('pdf')).toBe('pdf');
  });

  it('treats svg as text (XML, rendered via the rich/source path, not <img>-by-ext)', () => {
    expect(previewMediaKind('svg')).toBe('text');
  });

  it('treats source/unknown/extensionless files as text', () => {
    for (const ext of ['ts', 'rs', 'md', 'json', 'lock', '']) {
      expect(previewMediaKind(ext)).toBe('text');
    }
  });
});

describe('isEditableDataFile', () => {
  it('allows text data files', () => {
    expect(isEditableDataFile('artifacts/report.md')).toBe(true);
    expect(isEditableDataFile('knowhow/guide.md')).toBe(true);
    expect(isEditableDataFile('config/apis.json')).toBe(true);
    expect(isEditableDataFile('scripts/foo.py')).toBe(true);
    expect(isEditableDataFile('artifacts/data.csv')).toBe(true);
  });

  it('allows svg (text, previewed as image by default)', () => {
    expect(isEditableDataFile('artifacts/diagram.svg')).toBe(true);
  });

  it('rejects binary previews', () => {
    expect(isEditableDataFile('artifacts/photo.png')).toBe(false);
    expect(isEditableDataFile('artifacts/doc.pdf')).toBe(false);
    expect(isEditableDataFile('artifacts/clip.mp4')).toBe(false);
    expect(isEditableDataFile('auth-modules/binance-hmac.wasm')).toBe(false);
  });

  it('rejects the read-only system-knowhow tree even for text files', () => {
    expect(isEditableDataFile('system-knowhow/best-practices.md')).toBe(false);
  });

  it('rejects files with no recognized extension', () => {
    expect(isEditableDataFile('artifacts/Dockerfile')).toBe(false);
    expect(isEditableDataFile('artifacts/LICENSE')).toBe(false);
  });

  it('is case-insensitive on the extension', () => {
    expect(isEditableDataFile('artifacts/README.MD')).toBe(true);
  });
});
