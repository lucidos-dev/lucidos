import { describe, it, expect } from 'vitest';
import {
  computeHeaderCollapse,
  iconsRowWidth,
  mobileCollapseCount,
  type HeaderCollapseInput,
} from './useHeaderActionCollapse';

// Pins the progressive-collapse math for the DESKTOP content-pane header's
// right icon cluster, plus the one rule its mobile counterpart has. Mobile
// measures nothing (see the hook, and the reason in ContentHeaderActions), so
// its whole decision is the count function at the bottom of this file.
//
// Real-ish numbers at the default 16px root: every header action is a
// 2.25rem (36px) .icon-btn.header-icon (so is the ⋯ trigger and the bell) and
// the flex gap is 0.25rem (4px).
//
// Most cases below feed `computeHeaderCollapse` a fixed leading width, which
// exercises the linear fit on its own terms. Real callers never pass a measured
// one: the centre box is CENTRED on the row, so they pass half of what it
// leaves. The geometry that follows from that is what the non-overlap invariant
// further down walks, clamp arm by clamp arm.
const ICON = 36;
const GAP = 4;
const NAV = 116;

/** App-UI mode: refresh, open-in-tab, fullscreen (nearest-title first) + bell. */
function appUiInput(overrides: Partial<HeaderCollapseInput>): HeaderCollapseInput {
  return {
    containerWidth: 800,
    leadingWidth: NAV,
    centreWidth: 150,
    actionWidths: [ICON, ICON, ICON],
    anchorWidth: ICON,
    moreWidth: ICON,
    gapPx: GAP,
    ...overrides,
  };
}

/** Container width that exactly hosts nav + gaps + the title + a given icon
 *  row — the boundary every scenario is phrased around. */
function exactWidth(input: HeaderCollapseInput, collapsed: number): number {
  return input.leadingWidth + input.gapPx + input.centreWidth + input.gapPx + iconsRowWidth(input, collapsed);
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

  it('the anchor is never part of the collapse: the minimal row still pays for ⋯ + bell', () => {
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

  it('no context actions (anchor only): nothing to collapse, title ellipsizes when tight', () => {
    const input = appUiInput({ actionWidths: [] });
    expect(computeHeaderCollapse({ ...input, containerWidth: 800 }))
      .toEqual({ collapsed: 0, titleEllipsized: false });
    expect(computeHeaderCollapse({ ...input, containerWidth: exactWidth(input, 0) - 1 }))
      .toEqual({ collapsed: 0, titleEllipsized: true });
  });

  it('an empty title still collapses icons when the row alone overflows', () => {
    const input = appUiInput({ centreWidth: 0 });
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

  // ── The structural invariant the centred box enforces, mirrored in math ──
  // The title cluster is a fixed span centred on the row, its width the CSS
  // clamp below (`.pane-header-content-title` in styles/panels/shell.css), and
  // the actions are right-aligned at the row's trailing edge. So the box's
  // right edge must stay at least a gap left of the cluster, at every container
  // width and through all three arms of the clamp.
  //
  // The reserve is sized for the widest cluster this row can hold, so the two
  // middle arms are slack by construction and the interesting one is the floor:
  // there the box stops shrinking and the cluster keeps coming, which is the
  // regime the collapse exists for.
  const SPAN = 320;                    // --desktop-nav-span, 20rem
  const MIN_SPAN = 128;                // --desktop-nav-min-span, 8rem
  const RESERVE = 3 * ICON + 4 * GAP;  // --content-side-reserve

  /** The rendered width of the centred box at a given container width: the CSS
   *  clamp, in JS. */
  function boxWidth(container: number): number {
    return Math.min(Math.max(MIN_SPAN, container - 2 * RESERVE), SPAN);
  }

  it('non-overlap invariant: the centred box\'s right edge never reaches the icon row', () => {
    const base = appUiInput({});
    // Down to where even ⋯ + the bell cannot fit beside a box already on its
    // min-span floor. That is ~296px here, well under the Canvas pane's own
    // 360px floor (MIN_CONTENT_PANE_REM), so no reachable pane width is left
    // out; below it nothing can hold the two apart and the box is clipped.
    for (let w = 900; w >= 300; w -= 1) {
      const centreWidth = boxWidth(w);
      const input = {
        ...base,
        containerWidth: w,
        centreWidth,
        leadingWidth: Math.max(0, (w - centreWidth) / 2),
      };
      const { collapsed, titleEllipsized } = computeHeaderCollapse(input);
      expect(collapsed).not.toBe(1);

      const iconsLeft = w - iconsRowWidth(input, collapsed);
      const boxRight = (w + centreWidth) / 2;
      expect(boxRight, `container ${w}`).toBeLessThanOrEqual(iconsLeft - input.gapPx + 0.6);

      // Nothing is clipped while the reserve still holds.
      expect(titleEllipsized, `container ${w}`).toBe(false);
    }
  });

  it('the reserve holds the widest cluster the row can carry, so mid-widths never fold', () => {
    // The claim --content-side-reserve is sized on: two context icons riding
    // the row plus the bell, and the two gaps between them, plus the two gaps
    // the fit model charges the centred box. A set of three or more folds whole
    // (alwaysCollapseFrom), so nothing wider ever stands here.
    const widest = appUiInput({ actionWidths: [ICON, ICON] });
    expect(iconsRowWidth(widest, 0) + 2 * GAP).toBe(RESERVE);

    // The clamp's middle arm: the box takes exactly what the two reserves
    // leave, so the cluster has exactly the reserve and fits without folding.
    for (const w of [400, 450, 500, 550]) {
      const centreWidth = boxWidth(w);
      expect(centreWidth, `container ${w} is off the middle arm`).toBe(w - 2 * RESERVE);
      expect(computeHeaderCollapse({
        ...widest,
        containerWidth: w,
        centreWidth,
        leadingWidth: (w - centreWidth) / 2,
        alwaysCollapseFrom: 3,
      })).toEqual({ collapsed: 0, titleEllipsized: false });
    }
  });

  it('folds on the min-span arm, where the box stops giving way', () => {
    // A Canvas pane at its floor (360px) with two context actions: the box is
    // pinned at MIN_SPAN, so the cluster would reach it and the icons go into ⋯
    // instead. This is the whole reason the measurement survives the centring.
    const w = 360;
    const centreWidth = boxWidth(w);
    expect(centreWidth).toBe(MIN_SPAN);
    const input = {
      ...appUiInput({ actionWidths: [ICON, ICON] }),
      containerWidth: w,
      centreWidth,
      leadingWidth: (w - centreWidth) / 2,
      alwaysCollapseFrom: 3,
    };
    expect(computeHeaderCollapse(input).collapsed).toBe(2);
    // Unfolded it really would have reached the box: that is what it dodged.
    const unfoldedClearance = (w - iconsRowWidth(input, 0)) - (w + centreWidth) / 2;
    expect(unfoldedClearance).toBeLessThan(GAP);
  });

  // The thread pane's cluster has no permanent anchor: nothing after the
  // actions, so the minimal state is the ⋯ trigger alone and the anchor costs
  // neither its own width nor a gap to it.
  it('a cluster with no anchor pays for neither the anchor nor a gap to it', () => {
    const input = appUiInput({ anchorWidth: 0 });
    expect(iconsRowWidth(input, 0)).toBe(3 * ICON + 2 * GAP);
    expect(iconsRowWidth(input, 3)).toBe(ICON);
    // ...and it still walks the same steps as it narrows.
    const seen: number[] = [];
    for (let w = exactWidth(input, 0) + 10; w >= exactWidth(input, 3) - 10; w -= 1) {
      const { collapsed } = computeHeaderCollapse({ ...input, containerWidth: w });
      if (seen[seen.length - 1] !== collapsed) seen.push(collapsed);
    }
    expect(seen).toEqual([0, 2, 3]);
  });

  // The content pane's rule: past a couple of context icons the row stops
  // reading as "what I can do here", so the whole set folds into ⋯ at any width
  // and the menu names each one in words.
  describe('alwaysCollapseFrom', () => {
    it('folds a set at or past the threshold whole, however roomy the row', () => {
      const input = appUiInput({ containerWidth: 4000, alwaysCollapseFrom: 3 });
      expect(computeHeaderCollapse(input)).toEqual({ collapsed: 3, titleEllipsized: false });
    });

    it('leaves a smaller set to the measurement', () => {
      const two = appUiInput({ actionWidths: [ICON, ICON], alwaysCollapseFrom: 3 });
      expect(computeHeaderCollapse({ ...two, containerWidth: 4000 }))
        .toEqual({ collapsed: 0, titleEllipsized: false });
      // ...and it still collapses when the room runs out.
      expect(computeHeaderCollapse({ ...two, containerWidth: exactWidth(two, 0) - 1 }).collapsed)
        .toBe(2);
    });

    it('still reports the centre zone giving way once the folded row does not fit', () => {
      const input = appUiInput({ alwaysCollapseFrom: 3 });
      const width = exactWidth(input, 3) - 1;
      expect(computeHeaderCollapse({ ...input, containerWidth: width }))
        .toEqual({ collapsed: 3, titleEllipsized: true });
    });
  });

  it('collapse count is monotonic as the container narrows (no flip-flop between steps)', () => {
    const input = appUiInput({ centreWidth: 200 });
    let prev = 0;
    for (let w = 900; w >= 100; w -= 1) {
      const { collapsed } = computeHeaderCollapse({ ...input, containerWidth: w });
      expect(collapsed).toBeGreaterThanOrEqual(prev);
      prev = collapsed;
    }
  });
});

// Mobile's whole decision, which reads no box at all.
describe('mobileCollapseCount', () => {
  it('folds a set of two or more, whatever the width', () => {
    expect(mobileCollapseCount(2)).toBe(2);
    expect(mobileCollapseCount(3)).toBe(3);
    expect(mobileCollapseCount(4)).toBe(4);
  });

  it('leaves a LONE action on the row: ⋯ would stand in the same box', () => {
    expect(mobileCollapseCount(1)).toBe(0);
  });

  it('has nothing to fold when the view carries no context actions', () => {
    expect(mobileCollapseCount(0)).toBe(0);
  });

  it('never leaves more than one control before the bell', () => {
    // The bound the centred cluster's edge reserve is sized against: whatever
    // the count, the trailing cluster is the ⋯ trigger or one action, plus the
    // bell. Two icon boxes, never three.
    for (let n = 0; n <= 6; n++) {
      const collapsed = mobileCollapseCount(n);
      const standing = n - collapsed + (collapsed > 0 ? 1 : 0);
      expect(standing, `${n} actions leave ${standing} controls`).toBeLessThanOrEqual(1);
    }
  });
});
