import { describe, it, expect } from 'vitest';
import {
  computeHeaderCollapse,
  iconsRowWidth,
  measuredCollapseInput,
  mobileCollapseCount,
  type HeaderCollapseInput,
} from './useHeaderActionCollapse';

const ICON = 36;
const GAP = 4;
const NAV = 116;

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
    expect(iconsRowWidth(input, 3)).toBe(ICON + GAP + ICON);
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

  it('a 4-action list walks 0 → 2 → 3 → 4 as the container narrows', () => {
    const input = appUiInput({ actionWidths: [ICON, ICON, ICON, ICON] });
    const seen: number[] = [];
    for (let w = exactWidth(input, 0) + 10; w >= exactWidth(input, 4) - 10; w -= 1) {
      const { collapsed } = computeHeaderCollapse({ ...input, containerWidth: w });
      if (seen[seen.length - 1] !== collapsed) seen.push(collapsed);
    }
    expect(seen).toEqual([0, 2, 3, 4]);
  });

  const SPAN = 320;
  const SPAN_MAX = 576;
  const MIN_SPAN = 2 * ICON + 2.1 * 16;
  const RESERVE = 2 * ICON + 3 * GAP;
  const UNFOLDED = 7 * ICON + 8 * GAP;

  const clamp = (min: number, value: number, max: number) => Math.max(min, Math.min(value, max));

  function boxWidth(container: number): number {
    return clamp(MIN_SPAN, container - 2 * RESERVE, clamp(SPAN, container - 2 * UNFOLDED, SPAN_MAX));
  }

  function fixedSpanBoxWidth(container: number): number {
    return clamp(MIN_SPAN, container - 2 * RESERVE, SPAN);
  }

  it('non-overlap invariant: the centred box\'s right edge never reaches the icon row', () => {
    const base = appUiInput({});
    for (let w = 2000; w >= 300; w -= 1) {
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

      expect(titleEllipsized, `container ${w}`).toBe(false);
    }
  });

  it('folds two optional icons before they displace the fixed title span', () => {
    const input = appUiInput({ actionWidths: [ICON, ICON] });
    expect(iconsRowWidth(input, 2) + 2 * GAP).toBe(RESERVE);
    for (const w of [400, 450, 500, 550]) {
      const centreWidth = boxWidth(w);
      expect(computeHeaderCollapse({
        ...input, containerWidth: w, centreWidth, leadingWidth: (w - centreWidth) / 2,
      })).toEqual({ collapsed: 2, titleEllipsized: false });
    }
  });

  it('a third icon rides only where the box leaves more than the reserve', () => {
    const three = appUiInput({ actionWidths: [ICON, ICON, ICON] });
    for (const w of [400, 450, 500, 550]) {
      const centreWidth = boxWidth(w);
      expect(computeHeaderCollapse({
        ...three, containerWidth: w, centreWidth, leadingWidth: (w - centreWidth) / 2,
      })).toEqual({ collapsed: 3, titleEllipsized: false });
    }
    const wide = 2000;
    const capped = boxWidth(wide);
    expect(computeHeaderCollapse({
      ...three, containerWidth: wide, centreWidth: capped, leadingWidth: (wide - capped) / 2,
    })).toEqual({ collapsed: 0, titleEllipsized: false });
  });

  it('the title grows on a wide pane, up to its cap', () => {
    expect(boxWidth(700), 'a mid-width pane keeps the shared span').toBe(SPAN);
    expect(boxWidth(1000)).toBe(1000 - 2 * UNFOLDED);
    expect(boxWidth(1000)).toBeGreaterThan(SPAN);
    expect(boxWidth(2000)).toBe(SPAN_MAX);
  });

  it('growing never folds an action the fixed span let ride, up to six', () => {
    for (let count = 0; count <= 6; count++) {
      const actions = appUiInput({ actionWidths: Array.from({ length: count }, () => ICON) });
      for (let w = 2000; w >= 360; w -= 1) {
        const at = (centreWidth: number) => computeHeaderCollapse({
          ...actions, containerWidth: w, centreWidth, leadingWidth: (w - centreWidth) / 2,
        }).collapsed;
        expect(at(boxWidth(w)), `${count} actions at ${w}`).toBe(at(fixedSpanBoxWidth(w)));
      }
    }
  });

  it('folds on the min-span arm, where the box stops giving way', () => {
    const w = 260;
    const centreWidth = boxWidth(w);
    expect(centreWidth).toBe(MIN_SPAN);
    const input = {
      ...appUiInput({ actionWidths: [ICON, ICON] }),
      containerWidth: w,
      centreWidth,
      leadingWidth: (w - centreWidth) / 2,
    };
    expect(computeHeaderCollapse(input).collapsed).toBe(2);
    const unfoldedClearance = (w - iconsRowWidth(input, 0)) - (w + centreWidth) / 2;
    expect(unfoldedClearance).toBeLessThan(GAP);
  });

  it('a cluster with no anchor pays for neither the anchor nor a gap to it', () => {
    const input = appUiInput({ anchorWidth: 0 });
    expect(iconsRowWidth(input, 0)).toBe(3 * ICON + 2 * GAP);
    expect(iconsRowWidth(input, 3)).toBe(ICON);
    const seen: number[] = [];
    for (let w = exactWidth(input, 0) + 10; w >= exactWidth(input, 3) - 10; w -= 1) {
      const { collapsed } = computeHeaderCollapse({ ...input, containerWidth: w });
      if (seen[seen.length - 1] !== collapsed) seen.push(collapsed);
    }
    expect(seen).toEqual([0, 2, 3]);
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

describe('measuredCollapseInput', () => {
  const row = { clientWidth: 570, titleWidth: 320, iconWidth: ICON, actionCount: 2, anchorWidth: ICON, gapPx: GAP };

  it('keeps every action riding when nothing holds the trailing padding', () => {
    expect(computeHeaderCollapse(measuredCollapseInput({ ...row, trailingPadding: 0 })).collapsed).toBe(0);
  });

  it('folds sooner while the docked dashboard rail toggle holds the trailing padding', () => {
    const docked = measuredCollapseInput({ ...row, trailingPadding: ICON + GAP });
    expect(docked.leadingWidth).toBe((570 - 320) / 2);
    expect(computeHeaderCollapse(docked).collapsed).toBe(2);
  });
});

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
    for (let n = 0; n <= 6; n++) {
      const collapsed = mobileCollapseCount(n);
      const standing = n - collapsed + (collapsed > 0 ? 1 : 0);
      expect(standing, `${n} actions leave ${standing} controls`).toBeLessThanOrEqual(1);
    }
  });
});
