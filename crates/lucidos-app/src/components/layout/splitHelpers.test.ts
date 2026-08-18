import { describe, it, expect } from 'vitest';
import {
  clampToRange,
  clampSplitRatio,
  migratedSplitRatio,
  computeStepRatio,
  computeDrawerStepWidth,
  DEFAULT_SPLIT_RATIO,
  KEYBOARD_RESIZE_STEP_PX,
} from './splitHelpers';
import { minDrawerWidth, minThreadPanePx, minContentPanePx } from '../../store/paneMinimums';

const TOTAL = 1000;
// The pane floors are derived from the root font size now (paneMinimums.ts), so
// read them rather than restating the retired 300 / 360 constants. The harness
// answers a 16px root, where they are 338 and 360.
const MIN_THREAD = minThreadPanePx();
const MIN_CONTENT = minContentPanePx();
const BOUNDS = { minThreadPx: MIN_THREAD, minContentPx: MIN_CONTENT };

describe('clampToRange', () => {
  it('clamps inside a normal range', () => {
    expect(clampToRange(50, 10, 90)).toBe(50);
    expect(clampToRange(5, 10, 90)).toBe(10);
    expect(clampToRange(200, 10, 90)).toBe(90);
  });

  it('keeps the LEADING side whole when the range is empty', () => {
    // hi < lo: the container cannot hold both minimums. A bare
    // min-then-max would answer `hi` and hand the space to the trailing pane.
    expect(clampToRange(0, 300, 200)).toBe(300);
    expect(clampToRange(1000, 300, 200)).toBe(300);
  });
});

describe('migratedSplitRatio: a persisted ratio the floors have outgrown', () => {
  // The upgrade path. Raising the Conversation floor made every stored ratio
  // between the old value and the new illegal, and nothing re-clamps on load.
  // Without this, a user opens straight back into the overlapping header the
  // floor was raised to prevent.

  it('leaves a legal ratio exactly where the user put it', () => {
    // Null, not the same number: the caller must write and persist nothing.
    expect(migratedSplitRatio(0.5, TOTAL, BOUNDS)).toBeNull();
    expect(migratedSplitRatio(MIN_THREAD / TOTAL, TOTAL, BOUNDS)).toBeNull();
    expect(migratedSplitRatio((TOTAL - MIN_CONTENT) / TOTAL, TOTAL, BOUNDS)).toBeNull();
  });

  it('raises one that is under the Conversation floor to exactly the floor', () => {
    // 300px was the old floor, and is the width the reported overlap sat at.
    expect(migratedSplitRatio(300 / TOTAL, TOTAL, BOUNDS)).toBe(MIN_THREAD / TOTAL);
    expect(migratedSplitRatio(0.01, TOTAL, BOUNDS)).toBe(MIN_THREAD / TOTAL);
  });

  it('pulls one back off the Canvas pane at the other end', () => {
    expect(migratedSplitRatio(0.99, TOTAL, BOUNDS)).toBe((TOTAL - MIN_CONTENT) / TOTAL);
  });

  it('leaves a COLLAPSED pane alone: that is a state, not an illegal width', () => {
    // The toggles and the maximize shortcut put the ratio at exactly 0 or 1 on
    // purpose, and a drag cannot reach either (ADR 0056). Migrating one would
    // silently un-collapse a pane the user collapsed.
    expect(migratedSplitRatio(0, TOTAL, BOUNDS)).toBeNull();
    expect(migratedSplitRatio(1, TOTAL, BOUNDS)).toBeNull();
  });

  it('does nothing before the split has a width', () => {
    // `clampSplitRatio` answers DEFAULT_SPLIT_RATIO for an unmeasurable
    // container, which as a migration would be a silent reset of the layout.
    for (const total of [0, 1, 2]) {
      expect(migratedSplitRatio(0.01, total, BOUNDS), `total ${total}`).toBeNull();
    }
  });

  it('repairs a stored value that is not a number at all', () => {
    // `parseFloat` of a corrupt localStorage entry is NaN, and every comparison
    // against NaN is false, so it would otherwise flow through as a ratio.
    expect(migratedSplitRatio(NaN, TOTAL, BOUNDS)).toBe(DEFAULT_SPLIT_RATIO);
  });
});

describe('clampSplitRatio: a dragged split divider stops at the wall', () => {
  it('follows the pointer between the two minimums', () => {
    expect(clampSplitRatio(500, TOTAL, BOUNDS)).toBe(0.5);
    expect(clampSplitRatio(MIN_THREAD, TOTAL, BOUNDS)).toBe(MIN_THREAD / TOTAL);
  });

  it('stops at the thread pane minimum however far past it the pointer goes', () => {
    expect(clampSplitRatio(MIN_THREAD - 1, TOTAL, BOUNDS)).toBe(MIN_THREAD / TOTAL);
    expect(clampSplitRatio(0, TOTAL, BOUNDS)).toBe(MIN_THREAD / TOTAL);
    expect(clampSplitRatio(-9999, TOTAL, BOUNDS)).toBe(MIN_THREAD / TOTAL);
  });

  it('stops at the content pane minimum at the other end', () => {
    const ceil = (TOTAL - MIN_CONTENT) / TOTAL;
    expect(clampSplitRatio(TOTAL - MIN_CONTENT + 1, TOTAL, BOUNDS)).toBe(ceil);
    expect(clampSplitRatio(TOTAL, TOTAL, BOUNDS)).toBe(ceil);
    expect(clampSplitRatio(9999, TOTAL, BOUNDS)).toBe(ceil);
  });

  it('NEVER reaches 0 or 1, which is what keeps a collapse unreachable mid-drag', () => {
    for (const px of [-9999, -1, 0, 1, TOTAL / 2, TOTAL - 1, TOTAL, 9999]) {
      const r = clampSplitRatio(px, TOTAL, BOUNDS);
      expect(r, `pointer ${px}`).toBeGreaterThan(0);
      expect(r, `pointer ${px}`).toBeLessThan(1);
    }
  });

  it('holds that guarantee even when the container is SMALLER than a minimum', () => {
    // The case the pane-minimum clamp alone gets wrong, and it is reachable: a
    // drawer width persisted from a wider window can leave the split at 520px
    // while a 175% root puts the thread minimum at 525. `clampToRange` hands
    // back 525, and 525/520 rounds to exactly 1, collapsing the content pane
    // under the pointer. ADR 0056's whole argument depends on this not happening.
    const bounds = { minThreadPx: 525, minContentPx: 630 };
    for (const total of [520, 400, 100, 10, 3]) {
      for (const px of [-500, 0, total / 2, total, 5000]) {
        const r = clampSplitRatio(px, total, bounds);
        expect(r, `total ${total}, pointer ${px}`).toBeGreaterThan(0);
        expect(r, `total ${total}, pointer ${px}`).toBeLessThan(1);
      }
    }
  });

  it('answers the default for a container too small for "inside" to mean anything', () => {
    for (const total of [0, 1, 2]) {
      expect(clampSplitRatio(1, total, BOUNDS), `total ${total}`).toBe(0.4);
    }
  });

  it('degrades deterministically when the container cannot fit both minimums', () => {
    // 175% ui-scale on a narrow split: thread 525 + content 630 against 800.
    const total = 800;
    const bounds = { minThreadPx: 525, minContentPx: 630 };
    for (const px of [-100, 0, 400, 800, 5000]) {
      const r = clampSplitRatio(px, total, bounds);
      expect(r, `pointer ${px}`).toBeGreaterThanOrEqual(0);
      expect(r, `pointer ${px}`).toBeLessThanOrEqual(1);
      expect(Number.isFinite(r), `pointer ${px}`).toBe(true);
    }
    // The leading pane keeps its minimum; the trailing one takes what is left.
    expect(clampSplitRatio(0, total, bounds)).toBe(525 / total);
  });

  it('answers the default rather than NaN before the container has a width', () => {
    expect(clampSplitRatio(100, 0, BOUNDS)).toBe(0.4);
  });
});

describe('the drawer divider drag, which clamps through the same two helpers', () => {
  it('stops at the drawer floor and at the thread pane ceiling', () => {
    // DrawerDivider clamps the pointer into [its floor, row - threadMin - content].
    expect(clampToRange(300, 200, 500)).toBe(300);
    expect(clampToRange(-9999, 200, 500)).toBe(200);
    expect(clampToRange(9999, 200, 500)).toBe(500);
  });

  it('keeps the split off ZERO when the row cannot hold the drawer AND both panes', () => {
    // The empty-range case from the drawer's side: the ceiling drops below the
    // floor, the drawer holds its floor, and the space left for the split can
    // come out at or below the content pane's held width. The remainder the
    // divider hands to `clampSplitRatio` is then <= 0, which used to write a
    // ratio of exactly 0 and collapse the thread pane under the pointer.
    const bounds = { minThreadPx: 525, minContentPx: 630 };
    for (const remainder of [0, -1, -400]) {
      const r = clampSplitRatio(remainder, 900, bounds);
      expect(r, `remainder ${remainder}`).toBeGreaterThan(0);
      expect(r, `remainder ${remainder}`).toBeLessThan(1);
    }
  });
});

describe('computeStepRatio — keyboard resize of the split divider', () => {
  const STEP = KEYBOARD_RESIZE_STEP_PX;

  it('steps the divider by the given delta in both directions', () => {
    expect(computeStepRatio(0.5, TOTAL, STEP, BOUNDS)).toBe((500 + STEP) / TOTAL);
    expect(computeStepRatio(0.5, TOTAL, -STEP, BOUNDS)).toBe((500 - STEP) / TOTAL);
  });

  it('clamps immediately at the thread-pane minimum instead of collapsing', () => {
    const nearMin = (MIN_THREAD + 10) / TOTAL;
    expect(computeStepRatio(nearMin, TOTAL, -STEP, BOUNDS)).toBe(MIN_THREAD / TOTAL);
    // Already at the minimum: nothing to do.
    expect(computeStepRatio(MIN_THREAD / TOTAL, TOTAL, -STEP, BOUNDS)).toBeNull();
  });

  it('clamps immediately at the content-pane minimum instead of collapsing', () => {
    const ceil = (TOTAL - MIN_CONTENT) / TOTAL;
    const nearCeil = (TOTAL - MIN_CONTENT - 10) / TOTAL;
    expect(computeStepRatio(nearCeil, TOTAL, STEP, BOUNDS)).toBe(ceil);
    expect(computeStepRatio(ceil, TOTAL, STEP, BOUNDS)).toBeNull();
  });

  it('reopens a collapsed pane at its minimum when stepping back in', () => {
    // Thread pane collapsed; widening reopens it at MIN_THREAD.
    expect(computeStepRatio(0, TOTAL, STEP, BOUNDS)).toBe(MIN_THREAD / TOTAL);
    // Content pane collapsed; narrowing the thread pane reopens content at its minimum.
    expect(computeStepRatio(1, TOTAL, -STEP, BOUNDS)).toBe((TOTAL - MIN_CONTENT) / TOTAL);
  });

  it('treats a collapsed pane as settled against steps pushing further out', () => {
    expect(computeStepRatio(0, TOTAL, -STEP, BOUNDS)).toBeNull();
    expect(computeStepRatio(1, TOTAL, STEP, BOUNDS)).toBeNull();
  });

  it('is a no-op while the container has no width or cannot fit both minimums', () => {
    expect(computeStepRatio(0.5, 0, STEP, BOUNDS)).toBeNull();
    expect(computeStepRatio(0.5, MIN_THREAD + MIN_CONTENT - 1, STEP, BOUNDS)).toBeNull();
  });
});

describe('computeDrawerStepWidth — keyboard resize of the thread drawer', () => {
  const STEP = KEYBOARD_RESIZE_STEP_PX;
  const MAX = 900;

  // The floor is a CALLER measurement now (derived from the root font size and
  // the desktop build), passed in like maxPx so this module stays pure math.
  const MIN = minDrawerWidth();

  it('steps the width by the given delta in both directions', () => {
    expect(computeDrawerStepWidth(400, STEP, MIN, MAX)).toBe(400 + STEP);
    expect(computeDrawerStepWidth(400, -STEP, MIN, MAX)).toBe(400 - STEP);
  });

  it('clamps at the given minimum instead of closing', () => {
    expect(computeDrawerStepWidth(MIN + 10, -STEP, MIN, MAX)).toBe(MIN);
    expect(computeDrawerStepWidth(MIN, -STEP, MIN, MAX)).toBeNull();
  });

  it('clamps at maxPx', () => {
    expect(computeDrawerStepWidth(MAX - 10, STEP, MIN, MAX)).toBe(MAX);
    expect(computeDrawerStepWidth(MAX, STEP, MIN, MAX)).toBeNull();
  });

  it('is a no-op when the row cannot host even a minimum-width drawer', () => {
    expect(computeDrawerStepWidth(300, STEP, MIN, MIN - 1)).toBeNull();
  });
});
