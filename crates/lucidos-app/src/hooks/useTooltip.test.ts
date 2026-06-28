import { describe, it, expect } from 'vitest';
import { isRedundantTooltip, isTouchSwipe, reanchorToTarget, computeTooltipAnchor, computeTooltipVerticalPlacement, parseTooltipRows } from './useTooltip';

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
    // Target moved 50px up (scroll down) → tooltip anchor moves 50px up too
    const offset = { x: 30, y: 10 };
    const rect = { left: 100, top: 150 }; // was top: 200, now top: 150
    expect(reanchorToTarget(rect, offset)).toEqual({ x: 130, y: 160 });
  });

  it('shifts anchor sideways when target scrolls horizontally', () => {
    const offset = { x: 5, y: 5 };
    const rect = { left: 50, top: 200 }; // was left: 100, now left: 50
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
  // A wide element — e.g. a thread drawer row. The tooltip must anchor to the
  // item's border, NOT the pointer: same anchor wherever the cursor sits.
  const row = { left: 100, width: 200, top: 50, bottom: 80 };

  it('anchors a non-follow element to its horizontal center regardless of pointer X', () => {
    const near = computeTooltipAnchor(row, 110, 60, false); // pointer near left edge
    const far = computeTooltipAnchor(row, 290, 60, false);  // pointer near right edge
    expect(near.anchorX).toBe(200); // left + width/2 = 100 + 100
    expect(far.anchorX).toBe(200);  // identical — does not follow the pointer
  });

  it('anchors a non-follow element vertically to its top/bottom border, not the pointer', () => {
    const a = computeTooltipAnchor(row, 110, 55, false);
    const b = computeTooltipAnchor(row, 110, 78, false);
    expect(a.anchorTop).toBe(50);    // rect.top, regardless of pointer Y
    expect(a.anchorBottom).toBe(80); // rect.bottom
    expect(b.anchorTop).toBe(50);
    expect(b.anchorBottom).toBe(80);
  });

  it('anchors a TALL element to its border too when it has not opted in', () => {
    // Regression: a tall wrapped-title drawer row (>100px) used to flip into
    // pointer-tracking and drop the tooltip inside itself. Without opt-in it must
    // still anchor to the border so the tooltip stays fully outside the row.
    const tallRow = { left: 100, width: 200, top: 50, bottom: 200 }; // 150px tall
    const anchor = computeTooltipAnchor(tallRow, 150, 130, false); // pointer mid-row
    expect(anchor.anchorX).toBe(200);   // element center, not pointer
    expect(anchor.anchorTop).toBe(50);  // top border, not pointer Y (which is inside)
    expect(anchor.anchorBottom).toBe(200);
  });

  it('lets an opted-in element (the split divider) follow the pointer', () => {
    const divider = { left: 600, width: 8, top: 0, bottom: 800 };
    const anchor = computeTooltipAnchor(divider, 604, 420, true);
    expect(anchor.anchorX).toBe(604);     // pointer X
    expect(anchor.anchorTop).toBe(420);   // pointer Y
    expect(anchor.anchorBottom).toBe(420);
  });
});

describe('computeTooltipVerticalPlacement', () => {
  it('places above when there is room above the safe inset', () => {
    // anchorTop 300, height 80 → aboveTop = 300 - 80 - 8 = 212, clear of safeTop
    const { top, above } = computeTooltipVerticalPlacement(300, 330, 80, 0, false);
    expect(above).toBe(true);
    expect(top).toBe(212);
  });

  it('flips below when placing above would hide it behind the notch', () => {
    // iOS Dynamic Island: safeTop ~59. A header title at anchorTop 73 with a 55px
    // tooltip lands at top=10 — technically positive, so the OLD `top < 8` guard
    // kept it above, behind the camera strip. The safe-top clamp flips it below.
    const { top, above } = computeTooltipVerticalPlacement(73, 110, 55, 59, false);
    expect(above).toBe(false);
    expect(top).toBe(118); // anchorBottom 110 + gap 8
  });

  it('keeps a near-top tooltip above on a notchless device (safeTop 0)', () => {
    // Desktop: same anchors, no inset → aboveTop = 73 - 55 - 8 = 10 ≥ 8 → above.
    const { top, above } = computeTooltipVerticalPlacement(73, 110, 55, 0, false);
    expect(above).toBe(true);
    expect(top).toBe(10);
  });

  it('forces below regardless of available room when forceBelow is set', () => {
    const { top, above } = computeTooltipVerticalPlacement(300, 330, 80, 0, true);
    expect(above).toBe(false);
    expect(top).toBe(338); // anchorBottom 330 + gap 8
  });
});
