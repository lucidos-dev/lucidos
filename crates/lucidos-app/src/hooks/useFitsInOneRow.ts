import { useState, useLayoutEffect } from 'preact/hooks';
import type { RefObject } from 'preact';
import { getRemPx } from '../utils/dom';

/** Pure width math, exported so the hook's decision can be unit-tested
 *  without ResizeObserver/MutationObserver/jsdom quirks. Sums each item's
 *  measured width plus `gapCount` gaps of `gapPx`, then compares to
 *  `containerWidth` (with a 0.5px sub-pixel rounding fudge).
 *  Empty list = trivially fits.
 *
 *  `gapCount` defaults to one gap between every adjacent item, which is right
 *  for a row whose every member sits in the same gapped flex container. A row
 *  built from several clusters passes its own count, since a gap it never
 *  declares is width the row does not spend. */
export function computeFitsInOneRow(
  itemWidths: readonly number[],
  containerWidth: number,
  gapPx: number,
  gapCount: number = Math.max(0, itemWidths.length - 1),
): boolean {
  if (itemWidths.length === 0) return true;
  let total = 0;
  for (const w of itemWidths) total += w;
  total += gapCount * gapPx;
  return total <= containerWidth + 0.5;
}

/** The width the items actually have, which is the container's CONTENT box.
 *  `clientWidth` is the padding box, so a padded row reports space no item can
 *  stand on: the composer's row is padded `0 0.75rem 0.5rem 0.5rem` and
 *  overstated itself by 1.25rem. */
export function contentWidthOf(el: HTMLElement): number {
  const cs = getComputedStyle(el);
  const padding = (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0);
  return Math.max(0, el.clientWidth - padding);
}

/**
 * How many adjacent pairs in the row actually carry a gap.
 *
 * Without `clusterSelector`, every pair does. With one, only pairs inside a
 * matched cluster do, and an item outside every cluster is charged nothing.
 * That holds only while the container itself declares no `gap`. A source scan
 * pins that for the one row using this
 * (`styles/__tests__/prompt-actions-row-gap-guard.test.ts`).
 *
 * The count comes from the CLUSTER, never from the items' immediate parents. A
 * stacked cluster splits its members across sub-rows. Counting per parent would
 * report fewer gaps than the unstacked row needs, unstack it, then stack it
 * again on the next measurement. Reading through the cluster keeps one answer
 * for both states.
 */
export function countGappedPairs(container: HTMLElement, clusterSelector?: string): number {
  if (!clusterSelector) {
    return Math.max(0, container.querySelectorAll('[data-row-item]').length - 1);
  }
  let pairs = 0;
  container.querySelectorAll<HTMLElement>(clusterSelector).forEach((cluster) => {
    pairs += Math.max(0, cluster.querySelectorAll('[data-row-item]').length - 1);
  });
  return pairs;
}

export interface FitsInOneRowOptions {
  /** Gap between adjacent items inside a gapped cluster, in rem. */
  gapRem?: number;
  /** Selector for the container's gapped cluster(s). See `countGappedPairs`. */
  gappedCluster?: string;
}

/**
 * Whether every `[data-row-item]` descendant of `containerRef` would fit in a
 * single row across the container's content width.
 *
 * Each item contributes its own measured width, so the answer is stable across
 * stacked and unstacked re-renders: an item keeps its intrinsic width whichever
 * subrow it lands in. The gaps are the ones the row really declares.
 *
 * `gapRem` is multiplied by the document root font size at measure time so
 * user font scaling and viewport zoom feed in directly, with no hard-coded px
 * thresholds and no viewport-width heuristics.
 *
 * Re-measures on container resize (ResizeObserver) and on subtree mutation
 * (MutationObserver); button labels can change at runtime ("Apply" to
 * "Apply & Restart") and any width change must trigger a recomputation.
 */
export function useFitsInOneRow(
  containerRef: RefObject<HTMLElement>,
  { gapRem = 0.5, gappedCluster }: FitsInOneRowOptions = {},
): boolean {
  const [fits, setFits] = useState(true);
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const measure = () => {
      const items = container.querySelectorAll<HTMLElement>('[data-row-item]');
      const widths: number[] = [];
      items.forEach((item) => {
        widths.push(item.getBoundingClientRect().width);
      });
      setFits(computeFitsInOneRow(
        widths,
        contentWidthOf(container),
        gapRem * getRemPx(),
        countGappedPairs(container, gappedCluster),
      ));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(container);
    const mo = new MutationObserver(measure);
    mo.observe(container, {
      childList: true,
      subtree: true,
      characterData: true,
    });
    return () => {
      ro.disconnect();
      mo.disconnect();
    };
  }, [containerRef, gapRem, gappedCluster]);
  return fits;
}
