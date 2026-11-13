import { describe, it, expect } from 'vitest';
import { rangeLabel } from './rangeLabel';

describe('rangeLabel', () => {
  // Pins user-facing copy: a range reads "low to high", with no dash.
  it('joins the two ends with "to"', () => {
    expect(rangeLabel('Jul 6, 2026', 'Oct 9, 2026')).toBe('Jul 6, 2026 to Oct 9, 2026');
    expect(rangeLabel('0', '0.3')).toBe('0 to 0.3');
  });
});
