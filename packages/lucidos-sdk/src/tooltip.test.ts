import { describe, it, expect } from 'vitest';
import {
  isRedundantTooltip,
  isTouchSwipe,
  reanchorToTarget,
  computeTooltipAnchor,
  computeTooltipVerticalPlacement,
  parseTooltipRows,
  isTooltipsOptedOut,
  hasForeignTooltip,
  splitTooltipShortcut,
  TOOLTIP_SHORTCUT_SEPARATOR,
} from './tooltip';

describe('splitTooltipShortcut', () => {
  it('returns plain text with no shortcut', () => {
    expect(splitTooltipShortcut('Refresh · 3 new')).toEqual({ text: 'Refresh · 3 new', shortcut: null });
  });

  it('splits the key combo off the text', () => {
    expect(splitTooltipShortcut(`Refresh${TOOLTIP_SHORTCUT_SEPARATOR}⌘R`)).toEqual({ text: 'Refresh', shortcut: '⌘R' });
  });

  it('keeps a shortcut tooltip from reading as redundant with its label', () => {
    expect(isRedundantTooltip('Refresh', `Refresh${TOOLTIP_SHORTCUT_SEPARATOR}⌘R`, false)).toBe(false);
  });
});

describe('isRedundantTooltip', () => {
  it('flags exact matches as redundant', () => {
    expect(isRedundantTooltip('Files', 'Files', false)).toBe(true);
  });

  it('treats trim/case differences as redundant', () => {
    expect(isRedundantTooltip('Files', ' files ', false)).toBe(true);
    expect(isRedundantTooltip('files', 'FILES', false)).toBe(true);
  });

  it('keeps tooltip when text differs', () => {
    expect(isRedundantTooltip('auth.rs: Fix login bug', 'Fix login bug', false)).toBe(false);
  });

  it('keeps tooltip when visibly truncated, even if text matches', () => {
    expect(isRedundantTooltip('very-long-file-name.tsx', 'very-long-file-name.tsx', true)).toBe(false);
  });
});

describe('isTouchSwipe', () => {
  it('returns false for a stationary tap', () => {
    expect(isTouchSwipe(100, 100, 100, 100)).toBe(false);
  });

  it('returns false for tiny finger jitter under threshold', () => {
    expect(isTouchSwipe(100, 100, 105, 103)).toBe(false);
  });

  it('returns true for a horizontal swipe past threshold', () => {
    expect(isTouchSwipe(100, 100, 200, 100)).toBe(true);
  });

  it('returns true for a vertical scroll past threshold', () => {
    expect(isTouchSwipe(100, 100, 100, 200)).toBe(true);
  });

  it('uses euclidean distance (diagonal counts)', () => {
    expect(isTouchSwipe(100, 100, 108, 108)).toBe(true);
  });
});

describe('reanchorToTarget', () => {
  it('returns the same anchor when target has not moved', () => {
    const offset = { x: 30, y: 10 };
    const rect = { left: 100, top: 200 };
    expect(reanchorToTarget(rect, offset)).toEqual({ x: 130, y: 210 });
  });

  it('shifts anchor down when target scrolls up', () => {
    const offset = { x: 30, y: 10 };
    const rect = { left: 100, top: 150 };
    expect(reanchorToTarget(rect, offset)).toEqual({ x: 130, y: 160 });
  });

  it('shifts anchor sideways when target scrolls horizontally', () => {
    const offset = { x: 5, y: 5 };
    const rect = { left: 50, top: 200 };
    expect(reanchorToTarget(rect, offset)).toEqual({ x: 55, y: 205 });
  });
});

describe('parseTooltipRows', () => {
  it('normalizes a label/value pair per row', () => {
    const rows = parseTooltipRows(JSON.stringify([
      { label: 'Status', value: 'Running', tone: 'running' },
      { label: 'You', value: '2m ago' },
    ]));
    expect(rows).toEqual([
      { label: 'Status', value: 'Running', tone: 'running' },
      { label: 'You', value: '2m ago' },
    ]);
  });

  it('keeps a tone only when present', () => {
    const rows = parseTooltipRows(JSON.stringify([{ label: 'You', value: '2m ago' }]));
    expect(rows[0].tone).toBeUndefined();
  });

  it('returns an empty list for malformed or non-array JSON', () => {
    expect(parseTooltipRows('not json')).toEqual([]);
    expect(parseTooltipRows('{}')).toEqual([]);
  });
});

describe('computeTooltipAnchor', () => {
  const row = { left: 100, width: 200, top: 50, bottom: 80 };

  it('anchors a non-follow element to its horizontal center regardless of pointer X', () => {
    const near = computeTooltipAnchor(row, 110, 60, false);
    const far = computeTooltipAnchor(row, 290, 60, false);
    expect(near.anchorX).toBe(200);
    expect(far.anchorX).toBe(200);
  });

  it('anchors a non-follow element vertically to its top/bottom border, not the pointer', () => {
    const a = computeTooltipAnchor(row, 110, 55, false);
    const b = computeTooltipAnchor(row, 110, 78, false);
    expect(a.anchorTop).toBe(50);
    expect(a.anchorBottom).toBe(80);
    expect(b.anchorTop).toBe(50);
    expect(b.anchorBottom).toBe(80);
  });

  it('anchors a TALL element to its border too when it has not opted in', () => {
    const tallRow = { left: 100, width: 200, top: 50, bottom: 200 };
    const anchor = computeTooltipAnchor(tallRow, 150, 130, false);
    expect(anchor.anchorX).toBe(200);
    expect(anchor.anchorTop).toBe(50);
    expect(anchor.anchorBottom).toBe(200);
  });

  it('lets an opted-in element (the split divider) follow the pointer', () => {
    const divider = { left: 600, width: 8, top: 0, bottom: 800 };
    const anchor = computeTooltipAnchor(divider, 604, 420, true);
    expect(anchor.anchorX).toBe(604);
    expect(anchor.anchorTop).toBe(420);
    expect(anchor.anchorBottom).toBe(420);
  });
});

describe('computeTooltipVerticalPlacement', () => {
  it('places above when there is room above the safe inset', () => {
    const { top, above } = computeTooltipVerticalPlacement(300, 330, 80, 0, false);
    expect(above).toBe(true);
    expect(top).toBe(212);
  });

  it('flips below when placing above would hide it behind the notch', () => {
    const { top, above } = computeTooltipVerticalPlacement(73, 110, 55, 59, false);
    expect(above).toBe(false);
    expect(top).toBe(118);
  });

  it('keeps a near-top tooltip above on a notchless device (safeTop 0)', () => {
    const { top, above } = computeTooltipVerticalPlacement(73, 110, 55, 0, false);
    expect(above).toBe(true);
    expect(top).toBe(10);
  });

  it('forces below regardless of available room when forceBelow is set', () => {
    const { top, above } = computeTooltipVerticalPlacement(300, 330, 80, 0, true);
    expect(above).toBe(false);
    expect(top).toBe(338);
  });
});

describe('isTooltipsOptedOut', () => {
  it('is off by default', () => {
    expect(isTooltipsOptedOut(null, null)).toBe(false);
  });

  it('honours the attribute on either <html> or <body>', () => {
    expect(isTooltipsOptedOut('off', null)).toBe(true);
    expect(isTooltipsOptedOut(null, 'off')).toBe(true);
  });

  it('ignores any other value, so a typo never silently disables tooltips', () => {
    expect(isTooltipsOptedOut('', '')).toBe(false);
    expect(isTooltipsOptedOut('false', 'no')).toBe(false);
    expect(isTooltipsOptedOut('on', 'on')).toBe(false);
  });
});

describe('hasForeignTooltip', () => {
  const ours = { id: 'ours' } as unknown as Element;
  const theirs = { id: 'theirs' } as unknown as Element;

  it('sees nothing when the page has no #tooltip at all', () => {
    expect(hasForeignTooltip([], null)).toBe(false);
  });

  it('reads any node found before ours exists as foreign', () => {
    expect(hasForeignTooltip([theirs], null)).toBe(true);
  });

  it('does not mistake our own node for a foreign one', () => {
    expect(hasForeignTooltip([ours], ours)).toBe(false);
  });

  it('finds a hand-rolled node appended after ours', () => {
    expect(hasForeignTooltip([ours, theirs], ours)).toBe(true);
  });
});
