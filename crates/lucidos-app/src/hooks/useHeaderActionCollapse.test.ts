import { describe, it, expect } from 'vitest';
import {
  computeHeaderCollapse,
  computeMobileHeaderCollapse,
  iconsRowWidth,
  type HeaderCollapseInput,
  type MobileHeaderCollapseInput,
} from './useHeaderActionCollapse';

// Pins the progressive-collapse math for the content-pane header's right icon
// cluster. Real-ish numbers at the default 16px root: every header action is a
// 2.25rem (36px) .icon-btn.header-icon (so is the ⋯ trigger and the bell), the
// flex gap is 0.25rem (4px), and panel-nav is 3 buttons + 2 gaps ≈ 116px.
const ICON = 36;
const GAP = 4;
const NAV = 116;

/** App-UI mode: refresh, open-in-tab, fullscreen (nearest-title first) + bell. */
function appUiInput(overrides: Partial<HeaderCollapseInput>): HeaderCollapseInput {
  return {
    containerWidth: 800,
    navWidth: NAV,
    titleWidth: 150,
    actionWidths: [ICON, ICON, ICON],
    bellWidth: ICON,
    moreWidth: ICON,
    gapPx: GAP,
    ...overrides,
  };
}

/** Container width that exactly hosts nav + gaps + the title + a given icon
 *  row — the boundary every scenario is phrased around. */
function exactWidth(input: HeaderCollapseInput, collapsed: number): number {
  return input.navWidth + input.gapPx + input.titleWidth + input.gapPx + iconsRowWidth(input, collapsed);
}

describe('computeHeaderCollapse', () => {
  it('roomy: everything fits, nothing collapses, no ⋯', () => {
    const r = computeHeaderCollapse(appUiInput({ containerWidth: 800 }));
    expect(r).toEqual({ collapsed: 0, titleEllipsized: false });
  });

  it('first squeeze collapses the TWO icons nearest the title (never exactly one)', () => {
    const input = appUiInput({});
    // One px too narrow for the full row → the first collapse step. Icons at
    // c=2: ⋯ + fullscreen + bell (3 items) vs 4 items at c=0 — one icon+gap
    // narrower, so the same width fits.
    const width = exactWidth(input, 0) - 1;
    const r = computeHeaderCollapse({ ...input, containerWidth: width });
    expect(r).toEqual({ collapsed: 2, titleEllipsized: false });
  });

  it('next squeeze pulls one more icon in — minimal state is ⋯ + bell', () => {
    const input = appUiInput({});
    const width = exactWidth(input, 2) - 1;
    const r = computeHeaderCollapse({ ...input, containerWidth: width });
    expect(r).toEqual({ collapsed: 3, titleEllipsized: false });
  });

  it('title ellipsizes ONLY after the icons are minimal', () => {
    const input = appUiInput({});
    const width = exactWidth(input, 3) - 1;
    const r = computeHeaderCollapse({ ...input, containerWidth: width });
    expect(r).toEqual({ collapsed: 3, titleEllipsized: true });
  });

  it('the bell is never part of the collapse — the minimal row still pays for ⋯ + bell', () => {
    const input = appUiInput({});
    // ⋯ + bell + 1 gap.
    expect(iconsRowWidth(input, 3)).toBe(ICON + GAP + ICON);
    // Full row: 3 actions + bell + 3 gaps.
    expect(iconsRowWidth(input, 0)).toBe(4 * ICON + 3 * GAP);
  });

  it('a single context action never collapses (⋯ would replace it 1:1) — the title gives way instead', () => {
    const input = appUiInput({ actionWidths: [ICON] });
    const width = exactWidth(input, 0) - 1;
    const r = computeHeaderCollapse({ ...input, containerWidth: width });
    expect(r).toEqual({ collapsed: 0, titleEllipsized: true });
  });

  it('no context actions (bell only): nothing to collapse, title ellipsizes when tight', () => {
    const input = appUiInput({ actionWidths: [] });
    expect(computeHeaderCollapse({ ...input, containerWidth: 800 }))
      .toEqual({ collapsed: 0, titleEllipsized: false });
    expect(computeHeaderCollapse({ ...input, containerWidth: exactWidth(input, 0) - 1 }))
      .toEqual({ collapsed: 0, titleEllipsized: true });
  });

  it('an empty title still collapses icons when the row alone overflows', () => {
    const input = appUiInput({ titleWidth: 0 });
    const width = exactWidth(input, 0) - 1;
    const r = computeHeaderCollapse({ ...input, containerWidth: width });
    expect(r.collapsed).toBe(2);
  });

  it('tolerates sub-pixel rounding within 0.5px at the exact-fit boundary', () => {
    const input = appUiInput({});
    expect(computeHeaderCollapse({ ...input, containerWidth: exactWidth(input, 0) - 0.4 }).collapsed).toBe(0);
    expect(computeHeaderCollapse({ ...input, containerWidth: exactWidth(input, 0) - 0.6 }).collapsed).toBe(2);
  });

  // File-preview diff mode carries 4 context actions (refresh, whole-file,
  // source-toggle, edit): the steps walk 0 → 2 → 3 → 4, one per squeeze.
  it('a 4-action list walks 0 → 2 → 3 → 4 as the container narrows', () => {
    const input = appUiInput({ actionWidths: [ICON, ICON, ICON, ICON] });
    const seen: number[] = [];
    for (let w = exactWidth(input, 0) + 10; w >= exactWidth(input, 4) - 10; w -= 1) {
      const { collapsed } = computeHeaderCollapse({ ...input, containerWidth: w });
      if (seen[seen.length - 1] !== collapsed) seen.push(collapsed);
    }
    expect(seen).toEqual([0, 2, 3, 4]);
  });

  // ── The structural invariant the flex layout enforces, mirrored in math ──
  // For every container width down to the point where nav + the MINIMAL icon
  // row alone fill the region (below that the flex-shrink:0 zones themselves
  // overflow — a state the pane minimum widths prevent and overflow:clip
  // guards), the title zone's right edge must sit at least a gap left of the
  // icon row, and the chosen collapse count must never be 1.
  it('non-overlap invariant: the title zone right edge never crosses the icon row left edge', () => {
    const input = appUiInput({ titleWidth: 260 });
    const minSensible = input.navWidth + 2 * input.gapPx
      + iconsRowWidth(input, input.actionWidths.length);
    for (let w = 900; w >= minSensible; w -= 1) {
      const { collapsed, titleEllipsized } = computeHeaderCollapse({ ...input, containerWidth: w });
      expect(collapsed).not.toBe(1);

      const iconsW = iconsRowWidth(input, collapsed);
      // The icons are right-aligned (the flex:1 title zone absorbs all slack).
      const iconsLeft = w - iconsW;
      // The title zone spans from nav+gap to icons-gap; the rendered title is
      // the natural width clamped to that zone (flex min-width:0 + ellipsis).
      const zoneWidth = Math.max(0, w - input.navWidth - 2 * input.gapPx - iconsW);
      const titleRight = input.navWidth + input.gapPx + Math.min(input.titleWidth, zoneWidth);
      expect(titleRight).toBeLessThanOrEqual(iconsLeft - input.gapPx + 0.6);

      // Ellipsis is reported only at the minimal icon state.
      if (titleEllipsized) expect(collapsed).toBe(input.actionWidths.length);
    }
  });

  it('collapse count is monotonic as the container narrows (no flip-flop between steps)', () => {
    const input = appUiInput({ titleWidth: 200 });
    let prev = 0;
    for (let w = 900; w >= 100; w -= 1) {
      const { collapsed } = computeHeaderCollapse({ ...input, containerWidth: w });
      expect(collapsed).toBeGreaterThanOrEqual(prev);
      prev = collapsed;
    }
  });
});

// ── Mobile content header: absolutely row-centered title, symmetric reserve ──
// The mobile header centres the title on the ROW MIDDLE (not a flex zone), so a
// long title clears the trailing action cluster only if a SYMMETRIC reserve
// (bounded by whichever cluster is nearer the centre) fits it — else the hook
// collapses the nearest-title actions into the ⋮ menu to widen the reserve, and
// only ellipsizes (still centred) when even fully collapsed can't host it.
//
// Numbers approximate a 393pt iPhone viewport at ui-scale 125 (20px root):
// icons/⋮/bell are 1.75rem ≈ 35px, the flex gap 0.25rem ≈ 5px; the leading
// cluster (hamburger + back/forward) ends ~130px in; ~13px trails the bell.
describe('computeMobileHeaderCollapse', () => {
  const ICON = 35;
  const GAP = 5;
  const ROW = 393;
  const LEAD = 130;         // leading cluster right edge
  const RIGHT_GAP = 13;     // bell right edge → row right edge

  /** App-UI mode: refresh, open-in-tab, fullscreen (nearest-title first) + bell. */
  function mobileInput(overrides: Partial<MobileHeaderCollapseInput>): MobileHeaderCollapseInput {
    return {
      rowWidth: ROW,
      leadingRight: LEAD,
      trailingRightGap: RIGHT_GAP,
      titleWidth: 40,
      actionWidths: [ICON, ICON, ICON],
      bellWidth: ICON,
      moreWidth: ICON,
      gapPx: GAP,
      ...overrides,
    };
  }

  /** Left edge of the trailing cluster at a given collapse count — the bound the
   *  centred title must never cross. */
  function trailingLeft(input: MobileHeaderCollapseInput, collapsed: number): number {
    return input.rowWidth - input.trailingRightGap - iconsRowWidth(input, collapsed);
  }

  it('short title fits centered with no collapse and no ellipsis', () => {
    const r = computeMobileHeaderCollapse(mobileInput({ titleWidth: 40 }));
    expect(r.collapsed).toBe(0);
    expect(r.titleEllipsized).toBe(false);
    expect(r.titleMaxWidth).toBeGreaterThanOrEqual(40);
  });

  it('the "Half Marathon" case: collapses two actions so the full title shows centered', () => {
    // ~130px title (13 chars at ui-scale 125) can't fit the tight c=0 reserve but
    // fits once refresh+open fold into ⋮ (trailing shrinks 4→3 icons).
    const r = computeMobileHeaderCollapse(mobileInput({ titleWidth: 130 }));
    expect(r.collapsed).toBe(2);
    expect(r.titleEllipsized).toBe(false);
    expect(r.titleMaxWidth).toBeGreaterThanOrEqual(130);
  });

  it('never collapses exactly one — the first squeeze takes the two nearest the title', () => {
    // A title just past the c=0 reserve collapses straight to 2 (⋮ replaces one
    // icon 1:1, so collapsing a single one buys nothing).
    for (let t = 0; t <= 400; t += 3) {
      expect(computeMobileHeaderCollapse(mobileInput({ titleWidth: t })).collapsed).not.toBe(1);
    }
  });

  it('an over-long title ellipsizes (still centered) at the fully-collapsed reserve', () => {
    // 200px > the max symmetric room even with only ⋮ + bell → truncate, but only
    // collapse as far as widens the box (here the leading cluster binds at c=2, so
    // it does NOT over-collapse to 3).
    const r = computeMobileHeaderCollapse(mobileInput({ titleWidth: 200 }));
    expect(r.collapsed).toBe(2);
    expect(r.titleEllipsized).toBe(true);
  });

  it('when the LEADING cluster binds, an unfittable title does NOT collapse icons for no gain', () => {
    // A very wide leading cluster caps the symmetric box regardless of trailing
    // collapse — so collapsing the trailing actions can't help and must not happen.
    const r = computeMobileHeaderCollapse(mobileInput({ leadingRight: 180, titleWidth: 300 }));
    expect(r.collapsed).toBe(0);
    expect(r.titleEllipsized).toBe(true);
  });

  it('empty title: no collapse, no ellipsis', () => {
    const r = computeMobileHeaderCollapse(mobileInput({ titleWidth: 0 }));
    expect(r.collapsed).toBe(0);
    expect(r.titleEllipsized).toBe(false);
  });

  it('collapse count is monotonic as the title grows (no flip-flop)', () => {
    let prev = 0;
    for (let t = 0; t <= 400; t += 1) {
      const { collapsed } = computeMobileHeaderCollapse(mobileInput({ titleWidth: t }));
      expect(collapsed).toBeGreaterThanOrEqual(prev);
      prev = collapsed;
    }
  });

  // The load-bearing guarantee: the symmetric max-width box, centred on the row
  // middle, NEVER crosses either cluster's inner edge — at any title width, any
  // action count, and (below) any ui-scale.
  it('non-overlap invariant: the centered title box clears BOTH clusters', () => {
    for (const actions of [[] as number[], [ICON], [ICON, ICON, ICON], [ICON, ICON, ICON, ICON]]) {
      for (let t = 0; t <= 500; t += 5) {
        const input = mobileInput({ actionWidths: actions, titleWidth: t });
        const { collapsed, titleMaxWidth, titleEllipsized } = computeMobileHeaderCollapse(input);
        expect(collapsed).not.toBe(1);
        // The PAINTED title (natural width clamped to the reserve, centred) is
        // what can overlap; a 0-width clamp paints nothing (empty title), so
        // assert edges only when something actually renders.
        const painted = Math.min(t, titleMaxWidth);
        if (painted > 0) {
          const center = input.rowWidth / 2;
          expect(center - painted / 2).toBeGreaterThanOrEqual(input.leadingRight - 0.6);
          expect(center + painted / 2).toBeLessThanOrEqual(trailingLeft(input, collapsed) + 0.6);
        }
        // A non-ellipsized result must actually host the full natural title.
        if (!titleEllipsized) expect(titleMaxWidth + 0.5).toBeGreaterThanOrEqual(t);
      }
    }
  });

  // The bug reproduces at 393pt across ui-scale 100/125/150 (16/20/24px root).
  // A "Half Marathon"-length title never paints under the icons at any of them —
  // it either fits centered after collapse or truncates centered, never overlaps.
  it('393pt viewport at ui-scale 100/125/150: long title never overlaps the icons', () => {
    for (const rem of [16, 20, 24]) {
      const icon = 1.75 * rem;
      const input: MobileHeaderCollapseInput = {
        rowWidth: 393,                        // device pts — fixed across scale
        leadingRight: 12 + 1.75 * rem + 0.25 * rem + 2 * (1.375 * rem) + 0.25 * rem,
        trailingRightGap: 0.625 * rem,
        titleWidth: 13 * 0.6 * rem,           // ~13 chars at the title font size
        actionWidths: [icon, icon, icon],     // app-ui: refresh, open, fullscreen
        bellWidth: icon,
        moreWidth: icon,
        gapPx: 0.25 * rem,
      };
      const { collapsed, titleMaxWidth } = computeMobileHeaderCollapse(input);
      const center = input.rowWidth / 2;
      expect(center - titleMaxWidth / 2).toBeGreaterThanOrEqual(input.leadingRight - 0.6);
      expect(center + titleMaxWidth / 2).toBeLessThanOrEqual(trailingLeft(input, collapsed) + 0.6);
    }
  });
});
