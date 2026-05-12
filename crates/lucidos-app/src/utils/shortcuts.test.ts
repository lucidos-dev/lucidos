import { describe, expect, it, vi } from 'vitest';

vi.mock('./platform', () => ({ isMac: true }));

import { SHORTCUTS, formatBinding, formatKey, tooltipWithShortcut } from './shortcuts';

describe('shortcuts catalog', () => {
  it('exposes every shortcut with at least one binding', () => {
    for (const [id, def] of Object.entries(SHORTCUTS)) {
      expect(def.bindings.length, `${id} has no bindings`).toBeGreaterThan(0);
      for (const binding of def.bindings) {
        expect(binding.keys.length, `${id} has empty binding`).toBeGreaterThan(0);
      }
    }
  });

  it('groups every shortcut into a known category', () => {
    const validCategories = new Set(['Navigation', 'View']);
    for (const [id, def] of Object.entries(SHORTCUTS)) {
      expect(validCategories.has(def.category), `${id} has invalid category ${def.category}`).toBe(true);
    }
  });
});

describe('formatKey (Mac)', () => {
  it.each([
    ['cmd', '⌘'],
    ['shift', '⇧'],
    ['alt', '⌥'],
    ['ctrl', '⌃'],
    ['k', 'K'],
    ['=', '='],
  ])('formats %s as %s', (input, expected) => {
    expect(formatKey(input)).toBe(expected);
  });
});

describe('formatBinding (Mac)', () => {
  it('joins modifiers without separator on Mac', () => {
    expect(formatBinding({ keys: ['cmd', 'shift', 'o'] })).toBe('⌘⇧O');
  });

  it('handles a bare letter binding', () => {
    expect(formatBinding({ keys: ['t'] })).toBe('T');
  });
});

describe('tooltipWithShortcut', () => {
  it('appends the formatted binding to the label', () => {
    expect(tooltipWithShortcut('Search', 'searchEverywhere')).toBe('Search · ⌘K');
  });

  it('joins multiple bindings with " or "', () => {
    expect(tooltipWithShortcut('New thread', 'newThread')).toBe('New thread · ⌃⇧O or C');
  });
});
