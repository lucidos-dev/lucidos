import { describe, it, expect } from 'vitest';
import { formatFileCount } from './formatFileCount';

describe('formatFileCount', () => {
  it('spells out zero as a state, not a quantity', () => {
    // A change reconciled to zero files has an empty Diff; "0 files" reads as
    // a broken card, "No file changes" tells the user to discard it.
    expect(formatFileCount(0)).toBe('No file changes');
  });

  it('singularizes one', () => {
    expect(formatFileCount(1)).toBe('1 file');
  });

  it('pluralizes everything else', () => {
    expect(formatFileCount(2)).toBe('2 files');
    expect(formatFileCount(32)).toBe('32 files');
  });
});
