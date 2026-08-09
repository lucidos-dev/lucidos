import { describe, it, expect } from 'vitest';
import {
  computeHeaderCollapse,
  iconsRowWidth,
  type HeaderCollapseInput,
} from './useHeaderActionCollapse';

// Pins the progressive-collapse math for the DESKTOP content-pane header's
// right icon cluster. There is no mobile counterpart to pin: that header
// collapses every action unconditionally, so it has no math (see the hook, and
// the reason in ContentHeaderActions).
//
// Real-ish numbers at the default 16px root: every header action is a
// 2.25rem (36px) .icon-btn.header-icon (so is the ⋯ trigger and the bell), the
// flex gap is 0.25rem (4px), and the leading zone is 3 buttons + 2 gaps ≈ 116px.
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

  // ── The structural invariant the flex layout enforces, mirrored in math ──
  // For every container width down to the point where nav + the MINIMAL icon
  // row alone fill the region (below that the flex-shrink:0 zones themselves
  // overflow — a state the pane minimum widths prevent and overflow:clip
  // guards), the title zone's right edge must sit at least a gap left of the
  // icon row, and the chosen collapse count must never be 1.
  it('non-overlap invariant: the title zone right edge never crosses the icon row left edge', () => {
    const input = appUiInput({ centreWidth: 260 });
    const minSensible = input.leadingWidth + 2 * input.gapPx
      + iconsRowWidth(input, input.actionWidths.length);
    for (let w = 900; w >= minSensible; w -= 1) {
      const { collapsed, titleEllipsized } = computeHeaderCollapse({ ...input, containerWidth: w });
      expect(collapsed).not.toBe(1);

      const iconsW = iconsRowWidth(input, collapsed);
      // The icons are right-aligned (the flex:1 title zone absorbs all slack).
      const iconsLeft = w - iconsW;
      // The title zone spans from nav+gap to icons-gap; the rendered title is
      // the natural width clamped to that zone (flex min-width:0 + ellipsis).
      const zoneWidth = Math.max(0, w - input.leadingWidth - 2 * input.gapPx - iconsW);
      const titleRight = input.leadingWidth + input.gapPx + Math.min(input.centreWidth, zoneWidth);
      expect(titleRight).toBeLessThanOrEqual(iconsLeft - input.gapPx + 0.6);

      // Ellipsis is reported only at the minimal icon state.
      if (titleEllipsized) expect(collapsed).toBe(input.actionWidths.length);
    }
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
