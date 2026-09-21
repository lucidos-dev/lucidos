/** The composer row's fold decision, as pure math.
 *
 *  The hook around it reads the DOM, which jsdom does not lay out. What is
 *  worth pinning is the ladder, the ⋯ trigger's own cost, and the property that
 *  makes the whole thing safe: the answer never grows as the row does.
 *
 *  Widths, not counts. The row's members are not one size, so which member
 *  folds decides how much room it buys.
 *
 *  Plan: `docs/plans/2026-09-19-the-composer-row-is-one-row.md`. */
import { describe, it, expect } from 'vitest';
import { computePromptCollapse, type FoldGroup } from './usePromptActionCollapse';

/** One icon box, and a pinned zone four boxes wide. Round numbers, so a
 *  container width reads directly as a box count. */
const ICON = 10;
const PINNED = 40;

/** Four icon-box members, the shape the left cluster has. */
const ICONS = [ICON, ICON, ICON, ICON];

/** Every member in the row proper, and no gap anywhere. That isolates the
 *  candidate ladder from the cluster arithmetic, which has its own block. */
const foldAt = (containerWidth: number, memberWidths: readonly number[] = ICONS): number =>
  computePromptCollapse({
    containerWidth,
    pinnedWidth: PINNED,
    memberWidths,
    memberGroups: memberWidths.map((): FoldGroup => 'row'),
    pinnedClusterCount: 1,
    gapPx: 0,
    moreWidth: ICON,
  });

describe('the composer row folds only when it has to', () => {
  it('folds nothing while every member fits', () => {
    expect(foldAt(PINNED + 4 * ICON)).toBe(0);
  });

  /** The ⋯ trigger replaces one icon box with another, so folding one icon
   *  buys the row nothing and costs the user a tap. No special case says so:
   *  the count that saves nothing does not fit either. */
  it('never folds exactly one icon', () => {
    for (let width = 0; width <= PINNED + 6 * ICON; width += 1) {
      expect(foldAt(width)).not.toBe(1);
    }
  });

  it('takes the two nearest members on its first step', () => {
    // One box short of the full row: two go, and the ⋯ stands in for them.
    expect(foldAt(PINNED + 3 * ICON)).toBe(2);
  });

  it('charges the ⋯ trigger a box of its own', () => {
    // Four members folded to two leaves ⋯ plus two, which is three boxes.
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
  /** This is the case the uniform-width model could not express, and the one
   *  the reported row was: a text button several icon boxes wide, beside icons
   *  that fit. Folding it alone is what the row needs. */
  const WIDE_FIRST = [ICON * 5, ICON, ICON];

  it('folds exactly one when that one is wider than the ⋯', () => {
    // Room for ⋯ plus the two icons, but not for the wide member beside them.
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

  /** Same trade as the never-fold-one-icon rule, at the other end. */
  it('leaves a lone icon standing, however narrow the row', () => {
    expect(foldAt(0, [ICON])).toBe(0);
  });

  /** A lone WIDE member is different: folding it saves real width, so the row
   *  does it rather than letting the member leave the box. */
  it('folds a lone wide member rather than overflow', () => {
    expect(foldAt(PINNED + 2 * ICON, [ICON * 5])).toBe(1);
  });
});

describe('a cluster member takes its gap with it', () => {
  /** The gap is counted from the CANDIDATE, never from the rendered row. Read
   *  from the DOM it would fall with the fold, and near the threshold that
   *  oscillates: folding frees a gap, the freed gap makes the unfolded
   *  candidate fit, and unfolding brings the gap back. */
  const GAP = 4;
  const clusterFoldAt = (containerWidth: number): number =>
    computePromptCollapse({
      containerWidth,
      pinnedWidth: PINNED,
      // Two cluster members beside the send button, so three boxes and two gaps
      // while nothing is folded.
      memberWidths: [ICON, ICON],
      memberGroups: ['cluster', 'cluster'],
      pinnedClusterCount: 1,
      gapPx: GAP,
      moreWidth: ICON,
    });

  it('charges a gap per adjacent pair the candidate leaves standing', () => {
    // Both standing: two members, the send, and the two gaps between them.
    expect(clusterFoldAt(PINNED + 2 * ICON + 2 * GAP)).toBe(0);
  });

  it('stops charging the gap of a member it folded away', () => {
    // One box and one gap short of holding both. Folding both leaves the ⋯
    // beside the send: one box, and one gap inside the cluster for the send.
    expect(clusterFoldAt(PINNED + ICON + GAP)).toBe(2);
  });

  /** The property the oscillation would break: never fold more as the row
   *  grows, gaps included. */
  it('stays monotone across the whole range', () => {
    let previous = Infinity;
    for (let width = 0; width <= PINNED + 6 * ICON + 4 * GAP; width += 1) {
      const folded = clusterFoldAt(width);
      expect(folded).toBeLessThanOrEqual(previous);
      previous = folded;
    }
  });
});
