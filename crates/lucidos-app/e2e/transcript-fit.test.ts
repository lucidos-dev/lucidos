import { describe, expect, it } from 'vitest';
import { overflowsPane } from './transcript-fit';

describe('overflowsPane', () => {
  it('treats a pixel of rounding as a fit', () => {
    expect(overflowsPane(600, 600)).toBe(false);
    expect(overflowsPane(601, 600)).toBe(false);
  });

  it('calls a few pixels of real overflow an overflow, not a short thread', () => {
    expect(overflowsPane(607, 600)).toBe(true);
  });
});
