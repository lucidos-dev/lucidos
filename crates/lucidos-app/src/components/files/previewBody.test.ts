import { describe, it, expect } from 'vitest';
import { dataPreviewBody, repoPreviewBody, previewExt, isImageLike } from './previewBody';

const read = (path: string, sourceToggle = false, editing = false) =>
  dataPreviewBody(path, { sourceToggle, editing });

describe('previewExt', () => {
  it('lowercases the last dotted segment', () => {
    expect(previewExt('artifacts/README.MD')).toBe('md');
  });

  it('returns the whole name when there is no dot', () => {
    expect(previewExt('Makefile')).toBe('makefile');
  });
});

describe('isImageLike', () => {
  it('covers the binary image list plus svg', () => {
    expect(isImageLike('png')).toBe(true);
    expect(isImageLike('svg')).toBe(true);
    expect(isImageLike('rs')).toBe(false);
  });
});

describe('dataPreviewBody', () => {
  it('shows code, JSON and plain text as line-numbered source', () => {
    expect(read('artifacts/main.rs')).toBe('source');
    expect(read('config/apis.json')).toBe('source');
    expect(read('artifacts/notes.txt')).toBe('source');
  });

  it('renders the rich types rather than their source', () => {
    expect(read('artifacts/report.md')).toBe('markdown');
    expect(read('artifacts/table.csv')).toBe('csv');
    expect(read('artifacts/page.html')).toBe('html');
    expect(read('artifacts/page.htm')).toBe('html');
    expect(read('artifacts/deck.slides')).toBe('slides');
  });

  it('switches a rich type to source when the Source toggle is on', () => {
    expect(read('artifacts/report.md', true)).toBe('source');
    expect(read('artifacts/page.html', true)).toBe('source');
    expect(read('artifacts/deck.slides', true)).toBe('source');
  });

  it('leaves a type with no rendered form alone when the toggle is on', () => {
    expect(read('artifacts/main.rs', true)).toBe('source');
    expect(read('artifacts/photo.png', true)).toBe('image');
  });

  it('points a media element at the binary kinds', () => {
    expect(read('artifacts/photo.png')).toBe('image');
    expect(read('artifacts/doc.pdf')).toBe('pdf');
    expect(read('artifacts/clip.mp4')).toBe('video');
    expect(read('artifacts/take.mp3')).toBe('audio');
  });

  it('shows svg as a picture until the Source toggle asks otherwise', () => {
    expect(read('artifacts/diagram.svg')).toBe('image');
    expect(read('artifacts/diagram.svg', true)).toBe('source');
  });

  it('offers no preview for a type nothing here can show', () => {
    expect(read('auth-modules/binance-hmac.wasm')).toBe('unsupported');
    expect(read('artifacts/LICENSE')).toBe('unsupported');
  });

  it('shows the editor for an editable file while editing', () => {
    expect(read('artifacts/notes.txt', false, true)).toBe('editor');
  });

  it('ignores the editing flag for a file the editor cannot write', () => {
    expect(read('system-knowhow/glossary.md', false, true)).toBe('markdown');
    expect(read('artifacts/photo.png', false, true)).toBe('image');
  });
});

describe('repoPreviewBody', () => {
  const readRepo = (path: string, sourceToggle = false) => repoPreviewBody(path, { sourceToggle });

  it('shows source for code and for the Source toggle', () => {
    expect(readRepo('src/main.rs')).toBe('source');
    expect(readRepo('README.md', true)).toBe('source');
  });

  it('renders the self-contained types', () => {
    expect(readRepo('README.md')).toBe('markdown');
    expect(readRepo('data/rows.csv')).toBe('csv');
  });

  it('renders an SVG as an image from the engine URL, and its source on request', () => {
    expect(readRepo('assets/logo.svg')).toBe('image');
    expect(readRepo('assets/logo.svg', true)).toBe('source');
  });

  it('shows repo HTML as source, never as a live render', () => {
    expect(readRepo('crates/lucidos-app/index.html')).toBe('source');
    expect(readRepo('page.htm')).toBe('source');
  });

  it('shows an unknown or extensionless repo file as source', () => {
    expect(readRepo('Makefile')).toBe('source');
    expect(readRepo('Cargo.lock')).toBe('source');
  });

  it('points a media element at the binary kinds', () => {
    expect(readRepo('docs/diagram.png')).toBe('image');
    expect(readRepo('docs/spec.pdf')).toBe('pdf');
  });
});
