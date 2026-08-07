import { describe, it, expect } from 'vitest';
import { toastColumns, toastLayout } from './toastColumns';

/** Regression: a toast raised in the thread pane lowered the toasts in the
 *  content pane, and vice versa. Every toast shared one flex column, so the
 *  per-pane pin was horizontal only and each toast still took a row away from
 *  the other pane's stack. */

const t = (id: number, pane?: 'thread' | 'content') => ({ id, pane });

describe('toastLayout', () => {
  it('splits only when both panes are on screen', () => {
    expect(toastLayout(false, 0.4)).toBe('split');
  });

  it('is single-column on mobile at any ratio (one pane fills the screen)', () => {
    expect(toastLayout(true, 0.4)).toBe('single');
    expect(toastLayout(true, 0)).toBe('single');
    expect(toastLayout(true, 1)).toBe('single');
  });

  it('collapses to the surviving pane, matching SplitLayout thresholds', () => {
    // threadCollapsed === ratio 0, contentCollapsed === ratio >= 1.
    expect(toastLayout(false, 0)).toBe('content-only');
    expect(toastLayout(false, 1)).toBe('thread-only');
    expect(toastLayout(false, 1.2)).toBe('thread-only');
  });

  it('keeps the split through a live drag (the ratio never reaches 0 or 1)', () => {
    expect(toastLayout(false, 0.002)).toBe('split');
    expect(toastLayout(false, 0.998)).toBe('split');
  });
});

describe('toastColumns', () => {
  it('gives each pane its own stack so neither displaces the other', () => {
    const cols = toastColumns([t(3, 'content'), t(2, 'thread'), t(1, 'content')], 'split');
    expect(cols.map((c) => c.pane)).toEqual(['thread', 'content']);
    expect(cols[0].items.map((i) => i.id)).toEqual([2]);
    expect(cols[1].items.map((i) => i.id)).toEqual([3, 1]);
  });

  it('keeps both columns mounted when one pane has no toasts', () => {
    const cols = toastColumns([t(1, 'thread')], 'split');
    expect(cols).toHaveLength(2);
    expect(cols[1].items).toEqual([]);
  });

  it('defaults a pane-less toast to the thread column', () => {
    const cols = toastColumns([t(1)], 'split');
    expect(cols[0].items.map((i) => i.id)).toEqual([1]);
    expect(cols[1].items).toEqual([]);
  });

  it('merges into one newest-first column when only one pane is visible', () => {
    for (const layout of ['single', 'thread-only', 'content-only'] as const) {
      const cols = toastColumns([t(3, 'content'), t(2, 'thread'), t(1, 'content')], layout);
      expect(cols).toHaveLength(1);
      expect(cols[0].items.map((i) => i.id)).toEqual([3, 2, 1]);
    }
  });

  it('positions the merged column over the surviving pane, and nowhere on mobile', () => {
    expect(toastColumns([t(1, 'thread')], 'thread-only')[0].pane).toBe('thread');
    expect(toastColumns([t(1, 'thread')], 'content-only')[0].pane).toBe('content');
    expect(toastColumns([t(1, 'thread')], 'single')[0].pane).toBeNull();
  });
});
