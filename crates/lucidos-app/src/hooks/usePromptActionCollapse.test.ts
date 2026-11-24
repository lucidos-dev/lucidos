import { describe, it, expect } from 'vitest';
import { computePromptCollapse, type FoldGroup } from './usePromptActionCollapse';

const ICON = 10;
const PINNED = 40;

const ICONS = [ICON, ICON, ICON, ICON];

const foldAt = (containerWidth: number, memberWidths: readonly number[] = ICONS): number =>
  computePromptCollapse({
    containerWidth,
    pinnedWidth: PINNED,
    memberWidths,
    memberGroups: memberWidths.map((): FoldGroup => 'row'),
    pinnedClusterCount: 1,
    gapPx: 0,
    moreWidth: ICON,
  }).folded;

describe('the composer row folds only when it has to', () => {
  it('folds nothing while every member fits', () => {
    expect(foldAt(PINNED + 4 * ICON)).toBe(0);
  });

  it('never folds exactly one icon', () => {
    for (let width = 0; width <= PINNED + 6 * ICON; width += 1) {
      expect(foldAt(width)).not.toBe(1);
    }
  });

  it('takes the two nearest members on its first step', () => {
    expect(foldAt(PINNED + 3 * ICON)).toBe(2);
  });

  it('charges the ⋯ trigger a box of its own', () => {
    expect(foldAt(PINNED + 3 * ICON - 1)).toBe(3);
    expect(foldAt(PINNED + 2 * ICON)).toBe(3);
  });

  it('folds the whole set into ⋯ when one box is all the room there is', () => {
    expect(foldAt(PINNED + ICON)).toBe(4);
  });

  it('gives the largest fold when even ⋯ does not fit, and the floor takes over', () => {
    expect(foldAt(0)).toBe(4);
  });
});

describe('a wide member is worth folding on its own', () => {
  const WIDE_FIRST = [ICON * 5, ICON, ICON];

  it('folds exactly one when that one is wider than the ⋯', () => {
    expect(foldAt(PINNED + 3 * ICON, WIDE_FIRST)).toBe(1);
  });

  it('still folds nothing when the row can hold the wide member', () => {
    expect(foldAt(PINNED + 7 * ICON, WIDE_FIRST)).toBe(0);
  });
});

describe('the decision is monotone, so the row cannot oscillate', () => {
  it('never folds more as the row grows', () => {
    let previous = Infinity;
    for (let width = 0; width <= PINNED + 8 * ICON; width += 1) {
      const folded = foldAt(width);
      expect(folded).toBeLessThanOrEqual(previous);
      previous = folded;
    }
  });
});

describe('a row with almost nothing to fold', () => {
  it('folds nothing when there are no members', () => {
    expect(foldAt(0, [])).toBe(0);
  });

  it('leaves a lone icon standing, however narrow the row', () => {
    expect(foldAt(0, [ICON])).toBe(0);
  });

  it('folds a lone wide member rather than overflow', () => {
    expect(foldAt(PINNED + 2 * ICON, [ICON * 5])).toBe(1);
  });
});

describe('a cluster member takes its gap with it', () => {
  const GAP = 4;
  const clusterFoldAt = (containerWidth: number): number =>
    computePromptCollapse({
      containerWidth,
      pinnedWidth: PINNED,
      memberWidths: [ICON, ICON],
      memberGroups: ['cluster', 'cluster'],
      pinnedClusterCount: 1,
      gapPx: GAP,
      moreWidth: ICON,
    }).folded;

  it('charges a gap per adjacent pair the candidate leaves standing', () => {
    expect(clusterFoldAt(PINNED + 2 * ICON + 2 * GAP)).toBe(0);
  });

  it('stops charging the gap of a member it folded away', () => {
    expect(clusterFoldAt(PINNED + ICON + GAP)).toBe(2);
  });

  it('stays monotone across the whole range', () => {
    let previous = Infinity;
    for (let width = 0; width <= PINNED + 6 * ICON + 4 * GAP; width += 1) {
      const folded = clusterFoldAt(width);
      expect(folded).toBeLessThanOrEqual(previous);
      previous = folded;
    }
  });
});

describe('the merge step comes before the member at `atFold` folds', () => {
  const MEMBERS = [ICON, ICON, ICON, ICON];
  const AT_FOLD = 1;
  const settle = (containerWidth: number, mergedWidth = ICON) =>
    computePromptCollapse({
      containerWidth,
      pinnedWidth: PINNED,
      memberWidths: MEMBERS,
      memberGroups: MEMBERS.map((): FoldGroup => 'row'),
      pinnedClusterCount: 1,
      gapPx: 0,
      moreWidth: ICON,
      merge: { first: 2, count: 2, width: mergedWidth, atFold: AT_FOLD },
    });

  it('draws the pair as separate boxes while everything fits', () => {
    expect(settle(PINNED + 4 * ICON)).toEqual({ folded: 0, merged: false });
  });

  it('merges the pair before the member at `atFold` folds', () => {
    expect(settle(PINNED + 3 * ICON)).toEqual({ folded: 1, merged: true });
  });

  it('folds the member at `atFold` only once the pair is merged', () => {
    for (let width = 0; width <= PINNED + 5 * ICON; width += 1) {
      const { folded, merged } = settle(width);
      if (folded > AT_FOLD) expect(merged).toBe(true);
    }
  });

  it('folds the merged pair whole, never half', () => {
    for (let width = 0; width <= PINNED + 5 * ICON; width += 1) {
      expect(settle(width).folded).not.toBe(3);
    }
    expect(settle(PINNED + ICON).folded).toBe(4);
  });

  it('spends the merged control\'s own width', () => {
    expect(settle(PINNED + 3 * ICON, 2 * ICON).folded).toBe(2);
  });

  it('never merges or folds more as the row grows', () => {
    let previous = { folded: Infinity, merged: true };
    for (let width = 0; width <= PINNED + 6 * ICON; width += 1) {
      const next = settle(width);
      expect(next.folded).toBeLessThanOrEqual(previous.folded);
      if (next.folded === previous.folded) expect(next.merged && !previous.merged).toBe(false);
      previous = next;
    }
  });

  it('counts the merged group as one member of the gapped cluster', () => {
    const GAP = 4;
    const clusterSettle = (containerWidth: number) =>
      computePromptCollapse({
        containerWidth,
        pinnedWidth: PINNED,
        memberWidths: MEMBERS,
        memberGroups: ['row', 'row', 'cluster', 'cluster'],
        pinnedClusterCount: 1,
        gapPx: GAP,
        moreWidth: ICON,
        merge: { first: 2, count: 2, width: ICON, atFold: AT_FOLD },
      });
    expect(clusterSettle(PINNED + 3 * ICON + GAP)).toEqual({ folded: 1, merged: true });
    expect(clusterSettle(PINNED + 3 * ICON + GAP - 1).folded).toBe(2);
  });
});
