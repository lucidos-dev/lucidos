import { describe, it, expect } from 'vitest';
import { formatFileCount } from './formatFileCount';

describe('formatFileCount', () => {
  it('spells out zero as a state, not a quantity', () => {
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
