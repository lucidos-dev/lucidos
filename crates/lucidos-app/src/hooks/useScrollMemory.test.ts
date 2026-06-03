import { describe, it, expect, beforeEach } from 'vitest';
import { parseSavedScroll, isFullyRestorable, hasSavedScroll, resetContentScroll, contentScrollKey } from './useScrollMemory';

describe('isFullyRestorable', () => {
  it('true when scrollable range covers the saved offset', () => {
    expect(isFullyRestorable(200, 1000, 500)).toBe(true);
  });

  it('true when saved exactly equals maxScroll', () => {
    expect(isFullyRestorable(500, 1000, 500)).toBe(true);
  });

  it('false when content has not grown enough yet', () => {
    expect(isFullyRestorable(300, 600, 500)).toBe(false);
  });

  it('false when content fits viewport (no scroll possible)', () => {
    expect(isFullyRestorable(200, 400, 500)).toBe(false);
  });

  it('true for saved=0 — restoring to top is always achievable', () => {
    // Distinguishes "user scrolled to top" (saved=0) from "no save" (key absent).
    // Without this, restore is skipped and ThreadView's auto-scroll snaps to bottom.
    expect(isFullyRestorable(0, 1000, 500)).toBe(true);
    expect(isFullyRestorable(0, 400, 500)).toBe(true);
  });

  it('false for negative saved values', () => {
    expect(isFullyRestorable(-10, 1000, 500)).toBe(false);
  });
});

describe('parseSavedScroll', () => {
  it('parses valid non-negative integer string', () => {
    expect(parseSavedScroll('250')).toBe(250);
  });

  it('parses 0', () => {
    expect(parseSavedScroll('0')).toBe(0);
  });

  it('returns null for null input', () => {
    expect(parseSavedScroll(null)).toBeNull();
  });

  it('returns null for empty string', () => {
    expect(parseSavedScroll('')).toBeNull();
  });

  it('returns null for non-numeric input', () => {
    expect(parseSavedScroll('abc')).toBeNull();
  });

  it('returns null for negative values', () => {
    expect(parseSavedScroll('-10')).toBeNull();
  });

  it('parses fractional values to integer', () => {
    // scrollTop is normally an integer, but be defensive on read
    expect(parseSavedScroll('250.7')).toBe(250);
  });

  it('returns null for NaN', () => {
    expect(parseSavedScroll('NaN')).toBeNull();
  });
});

describe('hasSavedScroll', () => {
  beforeEach(() => localStorage.clear());

  it('true when key holds a positive integer', () => {
    localStorage.setItem('k', '42');
    expect(hasSavedScroll('k')).toBe(true);
  });

  it('false when key absent', () => {
    expect(hasSavedScroll('missing')).toBe(false);
  });

  it('true when key holds 0 — user scrolled to top, distinct from no-save', () => {
    // Bug: scrolling to the very top cleared the key, so on remount
    // ThreadView's auto-scroll-to-bottom kicked in instead of restoring to 0.
    localStorage.setItem('k', '0');
    expect(hasSavedScroll('k')).toBe(true);
  });

  it('false when key holds garbage', () => {
    localStorage.setItem('k', 'abc');
    expect(hasSavedScroll('k')).toBe(false);
  });

  it('false when key is null (caller had no key)', () => {
    expect(hasSavedScroll(null)).toBe(false);
  });
});

describe('contentScrollKey', () => {
  it('matches the key shape ContentPane writes', () => {
    // Tests the contract between writer (ContentPane) and invalidators
    // (e.g., submitTrigger). If these drift, "reset on save" silently no-ops.
    expect(contentScrollKey('triggers')).toBe('lucidos-scroll-content-triggers');
  });
});

describe('resetContentScroll', () => {
  beforeEach(() => localStorage.clear());

  it('removes the saved offset for the view', () => {
    localStorage.setItem('lucidos-scroll-content-triggers', '500');
    resetContentScroll('triggers');
    expect(localStorage.getItem('lucidos-scroll-content-triggers')).toBeNull();
  });

  it('is a no-op when nothing is saved', () => {
    expect(() => resetContentScroll('triggers')).not.toThrow();
  });

  it('does not touch other views', () => {
    localStorage.setItem('lucidos-scroll-content-triggers', '500');
    localStorage.setItem('lucidos-scroll-content-apps', '200');
    resetContentScroll('triggers');
    expect(localStorage.getItem('lucidos-scroll-content-apps')).toBe('200');
  });
});
