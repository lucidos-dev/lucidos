import { describe, it, expect } from 'vitest';
import { isEditableDataFile } from './previewExts';

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
