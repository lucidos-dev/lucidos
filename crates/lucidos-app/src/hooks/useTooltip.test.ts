import { describe, it, expect } from 'vitest';
import { isRedundantTooltip } from './useTooltip';

describe('isRedundantTooltip', () => {
  it('flags exact matches as redundant', () => {
    expect(isRedundantTooltip('Files', 'Files', false)).toBe(true);
  });

  it('treats trim/case differences as redundant', () => {
    expect(isRedundantTooltip('Files', ' files ', false)).toBe(true);
    expect(isRedundantTooltip('files', 'FILES', false)).toBe(true);
  });

  it('keeps tooltip when text differs', () => {
    expect(isRedundantTooltip('auth.rs — Fix login bug', 'Fix login bug', false)).toBe(false);
  });

  it('keeps tooltip when visibly truncated, even if text matches', () => {
    expect(isRedundantTooltip('very-long-file-name.tsx', 'very-long-file-name.tsx', true)).toBe(false);
  });
});
