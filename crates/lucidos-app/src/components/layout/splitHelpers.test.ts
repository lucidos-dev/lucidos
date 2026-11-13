import { describe, it, expect } from 'vitest';
import {
  clampToRange,
  clampSplitRatio,
  computeDrawnLayout,
  computeDrawnSplitRatio,
  contentShownRatio,
  sidePanesFit,
  type LayoutRoom,
  type HeldSideYields,
  computeStepRatio,
  computeDrawerStepWidth,
  DEFAULT_SPLIT_RATIO,
  KEYBOARD_RESIZE_STEP_PX,
} from './splitHelpers';
import { minDrawerWidth, minThreadPanePx, minContentPanePx, type PaneFloors } from '../../store/paneMinimums';

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

describe('computeDrawnSplitRatio: the Canvas yields instead of squeezing', () => {
  const BOTH = MIN_THREAD + MIN_CONTENT;
  const canvasPx = (ratio: number, total: number) => total * (1 - ratio);

  it('draws a legal stored ratio exactly where the user put it', () => {
    expect(computeDrawnSplitRatio(0.5, TOTAL, BOUNDS, null)).toBe(0.5);
    expect(computeDrawnSplitRatio(MIN_THREAD / TOTAL, TOTAL, BOUNDS, null)).toBe(MIN_THREAD / TOTAL);
  });

  it('holds both floors when there is room for both', () => {
    // 300px was the old Conversation floor, and is where the reported overlap sat.
    expect(computeDrawnSplitRatio(300 / TOTAL, TOTAL, BOUNDS, null)).toBe(MIN_THREAD / TOTAL);
    expect(computeDrawnSplitRatio(0.99, TOTAL, BOUNDS, null)).toBe((TOTAL - MIN_CONTENT) / TOTAL);
  });

  it('draws the Canvas collapsed when the split cannot hold both floors', () => {
    for (const total of [BOTH - 1, MIN_CONTENT, MIN_THREAD, 100]) {
      expect(computeDrawnSplitRatio(0.4, total, BOUNDS, null), `total ${total}`).toBe(1);
    }
  });

  it('keeps both panes at exactly the sum of the floors', () => {
    const r = computeDrawnSplitRatio(0.4, BOTH, BOUNDS, null);
    expect(r).toBe(MIN_THREAD / BOTH);
    expect(canvasPx(r, BOTH)).toBeCloseTo(MIN_CONTENT, 6);
  });

  it('never draws an open Canvas below its floor', () => {
    for (const total of [3, 100, BOTH - 1, BOTH, BOTH + 1, 1600, 2400]) {
      for (const stored of [0.01, 0.2, 0.4, 0.7, 0.99]) {
        const r = computeDrawnSplitRatio(stored, total, BOUNDS, null);
        if (r < 1) expect(canvasPx(r, total), `total ${total}, stored ${stored}`).toBeGreaterThanOrEqual(MIN_CONTENT - 1e-6);
      }
    }
  });

  it('leaves a collapsed pane collapsed: that is a state, not a width', () => {
    expect(computeDrawnSplitRatio(0, TOTAL, BOUNDS, null)).toBe(0);
    expect(computeDrawnSplitRatio(1, TOTAL, BOUNDS, null)).toBe(1);
    expect(computeDrawnSplitRatio(0, BOTH - 100, BOUNDS, null)).toBe(0);
    expect(computeDrawnSplitRatio(1, BOTH - 100, BOUNDS, null)).toBe(1);
  });

  it('draws the stored ratio before the split has a width', () => {
    for (const total of [0, 1, 2]) {
      expect(computeDrawnSplitRatio(0.4, total, BOUNDS, null), `total ${total}`).toBe(0.4);
    }
  });

  it('holds the collapse state it is given, for a drag in flight', () => {
    // Open at drag start: the room ran out under the pointer, but no
    // attribute flips until release. Strictly inside (0, 1), as a drag needs.
    const squeezed = computeDrawnSplitRatio(0.4, BOTH - 100, BOUNDS, false);
    expect(squeezed).toBeGreaterThan(0);
    expect(squeezed).toBeLessThan(1);
    // Yielded at drag start: the room came back, still drawn collapsed.
    expect(computeDrawnSplitRatio(0.4, TOTAL * 2, BOUNDS, true)).toBe(1);
  });

  it('returns the stored ratio once the room comes back', () => {
    // The stored ratio is never rewritten, so re-widening restores it.
    const stored = 0.45;
    expect(computeDrawnSplitRatio(stored, BOTH - 1, BOUNDS, null)).toBe(1);
    expect(computeDrawnSplitRatio(stored, 1600, BOUNDS, null)).toBe(stored);
  });
});

describe('contentShownRatio: showing the Canvas always shows it', () => {
  it('restores the default split when both panes fit', () => {
    expect(contentShownRatio(true)).toBe(DEFAULT_SPLIT_RATIO);
  });

  it('swaps the panes when only one fits', () => {
    expect(contentShownRatio(false)).toBe(0);
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
      expect(clampSplitRatio(1, total, BOUNDS), `total ${total}`).toBe(DEFAULT_SPLIT_RATIO);
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
    expect(clampSplitRatio(100, 0, BOUNDS)).toBe(DEFAULT_SPLIT_RATIO);
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

describe('computeDrawnLayout: the Conversation is never squeezed below its floor', () => {
  // Floors at 100%, with New thread and Home leading the Conversation row.
  const FLOORS: PaneFloors = { drawerPx: 320, minThreadPx: 498, minContentPx: 360, railPx: 364 };
  const DIVIDER = 3;
  const FREE: HeldSideYields = { drawer: null, rail: null };
  const room = (windowPx: number, floors: PaneFloors = FLOORS, storedRatio = 0.4): LayoutRoom =>
    ({ windowPx, floors, dividerPx: DIVIDER, storedRatio });
  const open = (widthPx: number) => ({ wanted: true, widthPx });
  const shut = { wanted: false, widthPx: 400 };

  const BOTH_SIDES = FLOORS.minThreadPx + FLOORS.drawerPx + DIVIDER + FLOORS.railPx;
  const DRAWER_ONLY = FLOORS.minThreadPx + FLOORS.drawerPx + DIVIDER;
  const RAIL_ONLY = FLOORS.minThreadPx + FLOORS.railPx;

  const drawnSet = (windowPx: number, floors?: PaneFloors) => {
    const l = computeDrawnLayout(room(windowPx, floors), open(400), open(450), FREE);
    return `${l.drawer.drawn ? 'drawer' : '-'}+${l.rail.drawn ? 'rail' : '-'}`;
  };

  it('draws both side panes at their stored widths when the window holds them', () => {
    const l = computeDrawnLayout(room(1800), open(400), open(450), FREE);
    expect(l.drawer).toEqual({ drawn: true, widthPx: 400 });
    expect(l.rail).toEqual({ drawn: true, widthPx: 450 });
    expect(l.splitPx).toBe(1800 - 400 - DIVIDER - 450);
  });

  it('yields the rail before the drawer, at the exact boundary of each step', () => {
    expect(drawnSet(BOTH_SIDES)).toBe('drawer+rail');
    expect(drawnSet(BOTH_SIDES - 1)).toBe('drawer+-');
    expect(drawnSet(DRAWER_ONLY)).toBe('drawer+-');
    expect(drawnSet(DRAWER_ONLY - 1)).toBe('-+-');
  });

  it('yields the drawer, not the rail, when only a narrower rail fits', () => {
    const narrowRail = { ...FLOORS, railPx: 250 };
    const railOnly = narrowRail.minThreadPx + narrowRail.railPx;
    expect(railOnly).toBeLessThan(DRAWER_ONLY);
    expect(drawnSet(DRAWER_ONLY, narrowRail)).toBe('drawer+-');
    expect(drawnSet(DRAWER_ONLY - 1, narrowRail)).toBe('-+rail');
    expect(drawnSet(railOnly, narrowRail)).toBe('-+rail');
    expect(drawnSet(railOnly - 1, narrowRail)).toBe('-+-');
  });

  it('draws a pane only when its stored state asks for it', () => {
    const wide = room(2000);
    expect(computeDrawnLayout(wide, shut, open(450), FREE).drawer.drawn).toBe(false);
    expect(computeDrawnLayout(wide, open(400), shut, FREE).rail.drawn).toBe(false);
    expect(computeDrawnLayout(wide, shut, shut, FREE).splitPx).toBe(2000);
  });

  it('holds the drawer to leave the Conversation its floor and a drawn rail its own', () => {
    const l = computeDrawnLayout(room(1200), open(700), open(450), FREE);
    expect(l.drawer.widthPx).toBe(1200 - DIVIDER - FLOORS.minThreadPx - FLOORS.railPx);
    expect(l.rail.widthPx).toBe(FLOORS.railPx);
    expect(l.splitPx).toBe(FLOORS.minThreadPx);
  });

  it('holds a drawer wider than the window leaves room for, when the rail is hidden', () => {
    const l = computeDrawnLayout(room(900), open(700), shut, FREE);
    expect(l.drawer).toEqual({ drawn: true, widthPx: 900 - DIVIDER - FLOORS.minThreadPx });
    expect(l.splitPx).toBe(FLOORS.minThreadPx);
  });

  it('never draws the drawer below its own floor', () => {
    expect(computeDrawnLayout(room(1800), open(100), shut, FREE).drawer.widthPx).toBe(FLOORS.drawerPx);
  });

  it('holds the rail to leave the drawn drawer, the Conversation and the Canvas their floors', () => {
    // Decision 5: the hold counts the drawn drawer. Without a drawer the same
    // rail would draw wider.
    const withDrawer = computeDrawnLayout(room(1800), open(400), open(1100), FREE);
    expect(withDrawer.rail.widthPx).toBe(1800 - 400 - DIVIDER - FLOORS.minThreadPx - FLOORS.minContentPx);
    const alone = computeDrawnLayout(room(1800), shut, open(1100), FREE);
    expect(alone.rail.widthPx).toBe(1800 - FLOORS.minThreadPx - FLOORS.minContentPx);
  });

  it('never draws the rail below its own floor, though that leaves the Canvas no room', () => {
    const l = computeDrawnLayout(room(1000), shut, open(900), FREE);
    expect(l.rail.widthPx).toBe(FLOORS.railPx);
  });

  it('counts only the Conversation when the stored split shows no Canvas', () => {
    const l = computeDrawnLayout(room(1000, FLOORS, 1), shut, open(900), FREE);
    expect(l.rail.widthPx).toBe(1000 - FLOORS.minThreadPx);
  });

  it('draws no drawer beside a collapsed Conversation, and weighs the rail against the Canvas', () => {
    const l = computeDrawnLayout(room(1000, FLOORS, 0), open(400), open(900), FREE);
    expect(l.drawer.drawn).toBe(false);
    expect(l.rail).toEqual({ drawn: true, widthPx: 1000 - FLOORS.minContentPx });
    const tight = FLOORS.minContentPx + FLOORS.railPx - 1;
    expect(computeDrawnLayout(room(tight, FLOORS, 0), open(400), open(900), FREE).rail.drawn).toBe(false);
  });

  it('holds a pinned yield through a change of room, for a drag in flight', () => {
    // Drawn at drag start: the room ran out under the pointer, and both stay.
    const squeezed = computeDrawnLayout(room(DRAWER_ONLY), open(400), open(450), { drawer: false, rail: false });
    expect(squeezed.drawer.drawn).toBe(true);
    expect(squeezed.rail.drawn).toBe(true);
    // Yielded at drag start: the room came back, and both stay yielded.
    const roomy = computeDrawnLayout(room(2400), open(400), open(450), { drawer: true, rail: true });
    expect(roomy.drawer.drawn).toBe(false);
    expect(roomy.rail.drawn).toBe(false);
  });

  it('returns every drawn value to the stored one when the room comes back', () => {
    const stored = [open(420), open(480)] as const;
    const before = computeDrawnLayout(room(1900), ...stored, FREE);
    computeDrawnLayout(room(DRAWER_ONLY - 1), ...stored, FREE);
    expect(computeDrawnLayout(room(1900), ...stored, FREE)).toEqual(before);
    expect(before.drawer.widthPx).toBe(420);
    expect(before.rail.widthPx).toBe(480);
  });

  it('draws the stored state before the window has a width', () => {
    for (const windowPx of [0, 1, 2]) {
      const l = computeDrawnLayout(room(windowPx), open(400), open(450), FREE);
      expect(l.drawer, `window ${windowPx}`).toEqual({ drawn: true, widthPx: 400 });
      expect(l.rail, `window ${windowPx}`).toEqual({ drawn: true, widthPx: 450 });
    }
  });

  it('keeps the width a yielded pane would draw at, for its slide out', () => {
    const l = computeDrawnLayout(room(DRAWER_ONLY), open(400), open(450), FREE);
    expect(l.rail.drawn).toBe(false);
    expect(l.rail.widthPx).toBeGreaterThanOrEqual(FLOORS.railPx);
  });

  it('never squeezes a Conversation that is shown below its floor, at any scale, width or stored state', () => {
    for (const scale of [1, 1.25, 1.5, 1.75]) {
      const floors: PaneFloors = {
        drawerPx: Math.round(FLOORS.drawerPx * scale), railPx: Math.round(FLOORS.railPx * scale),
        minThreadPx: Math.round(FLOORS.minThreadPx * scale), minContentPx: Math.round(FLOORS.minContentPx * scale),
      };
      for (let windowPx = 769; windowPx <= 2400; windowPx += 13) {
        for (const drawerW of [floors.drawerPx, 440, 900]) {
          for (const railW of [floors.railPx, 520, 1100]) {
            for (const drawer of [open(drawerW), shut]) {
              for (const rail of [open(railW), shut]) {
                for (const storedRatio of [0.4, 1]) {
                  const l = computeDrawnLayout(room(windowPx, floors, storedRatio), drawer, rail, FREE);
                  const where = `scale ${scale}, window ${windowPx}, ratio ${storedRatio}, drawer ${drawerW} ${drawer.wanted}, rail ${railW} ${rail.wanted}`;
                  if (l.drawer.drawn) expect(l.drawer.widthPx, where).toBeGreaterThanOrEqual(floors.drawerPx);
                  if (l.rail.drawn) expect(l.rail.widthPx, where).toBeGreaterThanOrEqual(floors.railPx);
                  if (windowPx >= floors.minThreadPx) expect(l.splitPx, where).toBeGreaterThanOrEqual(floors.minThreadPx - 1e-6);
                }
              }
            }
          }
        }
      }
    }
  });

  it('sidePanesFit answers each step against the same floors', () => {
    expect(sidePanesFit(room(BOTH_SIDES), true, true)).toBe(true);
    expect(sidePanesFit(room(BOTH_SIDES - 1), true, true)).toBe(false);
    expect(sidePanesFit(room(RAIL_ONLY), false, true)).toBe(true);
    expect(sidePanesFit(room(RAIL_ONLY - 1), false, true)).toBe(false);
    expect(sidePanesFit(room(FLOORS.minContentPx + FLOORS.railPx, FLOORS, 0), false, true)).toBe(true);
  });
});
