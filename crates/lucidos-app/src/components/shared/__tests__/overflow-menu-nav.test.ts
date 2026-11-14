import { describe, it, expect } from 'vitest';
import { nextMenuIndex } from '../OverflowMenu';

// The ⋯ menu's roving-focus math: ↑/↓ wrap at both ends; Home/End jump to the
// edges; a no-focus start (-1) steps to the natural end for the direction.
describe('nextMenuIndex', () => {
  const N = 4; // e.g. Pin / Archive / Copy ref / Download

  it('ArrowDown advances and wraps past the last item', () => {
    expect(nextMenuIndex(0, N, 'ArrowDown')).toBe(1);
    expect(nextMenuIndex(2, N, 'ArrowDown')).toBe(3);
    expect(nextMenuIndex(3, N, 'ArrowDown')).toBe(0);
  });

  it('ArrowUp retreats and wraps before the first item', () => {
    expect(nextMenuIndex(3, N, 'ArrowUp')).toBe(2);
    expect(nextMenuIndex(1, N, 'ArrowUp')).toBe(0);
    expect(nextMenuIndex(0, N, 'ArrowUp')).toBe(3);
  });

  it('seeds from no-focus (-1) to the direction\'s natural end', () => {
    expect(nextMenuIndex(-1, N, 'ArrowDown')).toBe(0);
    expect(nextMenuIndex(-1, N, 'ArrowUp')).toBe(N - 1);
  });

  it('Home/End jump to the edges regardless of current', () => {
    expect(nextMenuIndex(2, N, 'Home')).toBe(0);
    expect(nextMenuIndex(1, N, 'End')).toBe(N - 1);
  });

  it('an empty menu has no target', () => {
    expect(nextMenuIndex(-1, 0, 'ArrowDown')).toBe(-1);
    expect(nextMenuIndex(0, 0, 'End')).toBe(-1);
  });

  it('an unhandled key holds the current index', () => {
    expect(nextMenuIndex(2, N, 'Enter')).toBe(2);
  });
});
