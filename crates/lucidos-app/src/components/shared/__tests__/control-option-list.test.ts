import { describe, it, expect } from 'vitest';
import { wrapHighlight, selectedOptionIndex, sectionHeadingAt } from '../ControlOptionList';

describe('wrapHighlight', () => {
  it('steps down within bounds', () => {
    expect(wrapHighlight(0, 3, 1)).toBe(1);
    expect(wrapHighlight(1, 3, 1)).toBe(2);
  });

  it('wraps from the last row to the first when stepping down', () => {
    expect(wrapHighlight(2, 3, 1)).toBe(0);
  });

  it('steps up within bounds', () => {
    expect(wrapHighlight(2, 3, -1)).toBe(1);
  });

  it('wraps from the first row to the last when stepping up', () => {
    expect(wrapHighlight(0, 3, -1)).toBe(2);
  });

  it('returns 0 for an empty list rather than a negative index', () => {
    expect(wrapHighlight(0, 0, 1)).toBe(0);
    expect(wrapHighlight(0, 0, -1)).toBe(0);
  });
});

describe('selectedOptionIndex', () => {
  const opts = [{ value: 'a' }, { value: 'b' }, { value: 'c' }];

  it('returns the index of the current value so the sub-menu opens on it', () => {
    expect(selectedOptionIndex(opts, 'b')).toBe(1);
  });

  it('falls back to 0 when the current value is absent', () => {
    // A stored model/effort that has since been disabled or removed must still
    // land on a valid row instead of -1 (which would highlight nothing).
    expect(selectedOptionIndex(opts, 'gone')).toBe(0);
  });

  it('falls back to 0 for an empty option list', () => {
    expect(selectedOptionIndex([], 'a')).toBe(0);
  });
});

describe('sectionHeadingAt', () => {
  const rows = [
    { value: 'a', label: 'A', section: 'Recommended' },
    { value: 'b', label: 'B', section: 'Recommended' },
    { value: 'c', label: 'C', section: 'Other models' },
  ];

  it('heads each run once, where it begins', () => {
    expect(rows.map((_, i) => sectionHeadingAt(rows, i))).toEqual(['Recommended', null, 'Other models']);
  });

  it('heads nothing in a list without sections', () => {
    expect(sectionHeadingAt([{ value: 'a', label: 'A' }], 0)).toBeNull();
  });
});
