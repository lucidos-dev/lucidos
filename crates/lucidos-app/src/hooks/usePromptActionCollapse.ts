import { useState, useLayoutEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';
import { getRemPx } from '../utils/dom';
import { computeFitsInOneRow, contentWidthOf } from './useFitsInOneRow';

/** Marks a foldable member and names which one. The name is what lets a width
 *  be remembered across the fold that removes the box it was measured on. The
 *  ⋯ trigger carries the marker with an EMPTY name, since it stands in for
 *  members rather than being one. */
export const FOLD_KEY_ATTR = 'data-fold-key';

/** The row's only gapped cluster. The row itself declares none, so its icon
 *  boxes touch (`styles/__tests__/prompt-actions-row-gap-guard.test.ts`). */
const GAPPED_CLUSTER = '.prompt-actions-right';

/** Where a foldable member renders. The cluster declares a gap and the rest of
 *  the row does not, so the two are not interchangeable in the arithmetic. */
export type FoldGroup = 'row' | 'cluster';

/** Inputs to the composer row's fold decision, all in px.
 *
 *  Widths, not counts. The row's members are not one size: an icon box sits
 *  beside an Apply split button several times its width. So which member folds
 *  decides how much room it buys. */
export interface PromptCollapseInput {
  /** The row's content width. */
  containerWidth: number;
  /** Everything that never folds, summed: the control menu, the send button and
   *  the gaps the right-hand cluster declares. */
  pinnedWidth: number;
  /** Each foldable member's width, in FOLD ORDER. The fold takes a prefix. */
  memberWidths: readonly number[];
  /** Which of those render inside the gapped cluster, by the same index. */
  memberGroups: readonly FoldGroup[];
  /** Members the cluster always holds: the send button, or the answer control
   *  standing in for it. Their widths are already in `pinnedWidth`; this is the
   *  count, which is what the gaps are drawn from. */
  pinnedClusterCount: number;
  /** The gap the cluster declares between two adjacent members. */
  gapPx: number;
  /** The ⋯ trigger's box, spent whenever anything is folded. */
  moreWidth: number;
}

/**
 * How many of the composer row's foldable members move into the ⋯ menu.
 *
 * The smallest count that fits wins, and every count from 0 to N is a
 * candidate. `computeHeaderCollapse` skips a fold of exactly one, because its
 * members are uniform icon boxes and folding one trades a box for the ⋯ box.
 * Here the arithmetic says that by itself. Folding one icon saves nothing and
 * will not fit; folding one wide button saves its width.
 *
 * The fold takes a PREFIX, so the count always names the members earliest in
 * fold order. The last one standing is the one nearest the send button.
 *
 * Nothing fits at all? Return N, every member folded. The caller pins only two
 * members, the control menu and the send button. Those fit at any width the app
 * supports, so that answer is always a row the box can hold.
 *
 * The cluster's GAPS are counted from the candidate, never from the DOM. A
 * folded cluster member takes its gap with it, so reading the rendered subtree
 * would make `pinnedWidth` depend on the current fold. Near the threshold that
 * oscillates: folding frees a gap, the freed gap makes the unfolded candidate
 * fit, and unfolding brings the gap back.
 */
export function computePromptCollapse(input: PromptCollapseInput): number {
  const {
    containerWidth, pinnedWidth, memberWidths, memberGroups,
    pinnedClusterCount, gapPx, moreWidth,
  } = input;
  const total = memberWidths.length;
  let best = 0;
  let bestWidth = Infinity;
  for (let c = 0; c <= total; c++) {
    let standing = 0;
    let inCluster = pinnedClusterCount;
    for (let i = c; i < total; i++) {
      standing += memberWidths[i];
      if (memberGroups[i] === 'cluster') inCluster++;
    }
    const width = (c > 0 ? moreWidth : 0) + standing + Math.max(0, inCluster - 1) * gapPx;
    // Two zones and no gap between them: the row declares none, so every gap in
    // play is already inside `pinnedWidth`. The shared helper still owns the
    // sub-pixel fudge.
    if (computeFitsInOneRow([pinnedWidth, width], containerWidth, 0, 0)) return c;
    // Strictly narrower, so a tie keeps the smaller count. That is what leaves
    // a lone icon standing when even the ⋯ would not fit: folding it swaps one
    // box for another and costs a tap.
    if (width < bestWidth) { bestWidth = width; best = c; }
  }
  return best;
}

/**
 * The composer row's fold count, measured.
 *
 * `useHeaderActionCollapse` cannot serve this row. It models a header as
 * leading, centre and actions, and its members are uniform. The composer's are
 * not, and it has no centred box. What the two share is `computeFitsInOneRow`.
 *
 * Width decides and nothing else: no breakpoint, and no fold-past-a-count rule.
 * A phone lands folded because the row genuinely does not fit, and a wide pane
 * keeps every glyph.
 *
 * **A measured width cannot lie, because no member may shrink.** Every member
 * of `.prompt-actions-row` is `flex-shrink: 0` (chat/input-messages.css), so a
 * laid-out width is the member's true width. Without that rule a button with a
 * `min-width` floor squeezes under pressure while its nowrap label spills. The
 * measurement then reads the squeezed box and folds nothing.
 *
 * **A folded member has no box, so its width is remembered.** The cache is
 * keyed by member and refreshed for every member standing. It resets whenever
 * the member SET changes, so each one is measured before anything folds. An
 * unmeasured member is assumed to be one ⋯ box, the smallest a member can be.
 * That can only under-fold, and the next pass corrects it.
 */
export function usePromptActionCollapse(
  containerRef: RefObject<HTMLElement>,
  memberKeys: readonly string[],
  memberGroups: readonly FoldGroup[],
  gapRem = 0.5,
): number {
  const [collapsed, setCollapsed] = useState(0);
  const widths = useRef<Map<string, number>>(new Map());
  const keys = memberKeys.join(' ');
  useLayoutEffect(() => {
    // The set changed, so a member may never have been measured. Start from a
    // fully standing row: `useLayoutEffect` runs before paint, so the pass that
    // measures it and the pass that folds it land in the same frame.
    widths.current = new Map();
    setCollapsed(0);
  }, [keys]);
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const measure = () => {
      const seen = widths.current;
      let pinnedWidth = 0;
      let moreWidth = 0;
      let pinnedClusterCount = 0;
      container.querySelectorAll<HTMLElement>('[data-row-item]').forEach((item) => {
        const width = item.getBoundingClientRect().width;
        const key = item.getAttribute(FOLD_KEY_ATTR);
        if (key === '') { moreWidth = width; return; }
        if (key !== null) { seen.set(key, width); return; }
        // Pinned. Its width counts, and inside the cluster so does its gap.
        pinnedWidth += width;
        if (item.closest(GAPPED_CLUSTER)) pinnedClusterCount++;
      });
      const fallback = moreWidth || seen.values().next().value || 0;
      setCollapsed(computePromptCollapse({
        containerWidth: contentWidthOf(container),
        pinnedWidth,
        memberWidths: memberKeys.map((k) => seen.get(k) ?? fallback),
        memberGroups,
        pinnedClusterCount,
        gapPx: gapRem * getRemPx(),
        moreWidth: moreWidth || fallback,
      }));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(container);
    const mo = new MutationObserver(measure);
    mo.observe(container, { childList: true, subtree: true, characterData: true });
    return () => {
      ro.disconnect();
      mo.disconnect();
    };
  }, [containerRef, keys, gapRem, memberKeys, memberGroups]);
  return collapsed;
}
