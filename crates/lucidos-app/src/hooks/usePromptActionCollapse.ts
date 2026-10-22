import { useState, useLayoutEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';
import { getRemPx } from '../utils/dom';
import { computeFitsInOneRow, contentWidthOf } from './useFitsInOneRow';

/** Marks a foldable member and names which one. The name is what lets a width
 *  be remembered across the fold that removes the box it was measured on. The
 *  ⋯ trigger carries the marker with an EMPTY name, since it stands in for
 *  members rather than being one. */
export const FOLD_KEY_ATTR = 'data-fold-key';

/** The row attribute naming how far the shedding member has shortened: `0`
 *  full, up to its last level. CSS keys the member's faces off it. */
export const SHED_ATTR = 'data-shed';

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
  /** Everything that never folds, summed: the send button (or the answer
   *  control) and the gaps the right-hand cluster declares. */
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
  /** The one member that shortens before anything folds: its index in fold
   *  order, and its width at each shed level, full first. */
  shedding?: { index: number; widths: readonly number[] } | null;
}

/** Where the row settles: how many members fold, and how far the shedding
 *  member has shortened. */
export interface PromptCollapse {
  folded: number;
  shed: number;
}

const STANDING: PromptCollapse = { folded: 0, shed: 0 };

/**
 * How many of the composer row's foldable members move into the ⋯ menu, and
 * how far the shedding member shortens first.
 *
 * Candidates run in order and the first that fits wins. The shedding member
 * walks its levels with nothing folded, then every fold count from 1 to N
 * follows at its last level. So no member folds while one can still shorten.
 *
 * `computeHeaderCollapse` skips a fold of exactly one, because its members are
 * uniform icon boxes and folding one trades a box for the ⋯ box. Here the
 * arithmetic says that by itself. Folding one icon saves nothing and will not
 * fit; folding one wide button saves its width.
 *
 * The fold takes a PREFIX, so the count always names the members earliest in
 * fold order. The last one standing is the one nearest the send button.
 *
 * Nothing fits at all? Return the narrowest candidate, a tie keeping the
 * earlier one. The caller pins only the send button, or the answer control
 * standing in for it. That fits at any width the app supports.
 *
 * The cluster's GAPS are counted from the candidate, never from the DOM. A
 * folded cluster member takes its gap with it, so reading the rendered subtree
 * would make `pinnedWidth` depend on the current fold. Near the threshold that
 * oscillates: folding frees a gap, the freed gap makes the unfolded candidate
 * fit, and unfolding brings the gap back.
 */
export function computePromptCollapse(input: PromptCollapseInput): PromptCollapse {
  const {
    containerWidth, pinnedWidth, memberWidths, memberGroups,
    pinnedClusterCount, gapPx, moreWidth, shedding = null,
  } = input;
  const total = memberWidths.length;
  const lastShed = shedding ? shedding.widths.length - 1 : 0;
  const candidates: PromptCollapse[] = [];
  for (let shed = 0; shed < lastShed; shed++) candidates.push({ folded: 0, shed });
  for (let folded = 0; folded <= total; folded++) candidates.push({ folded, shed: lastShed });

  let best = candidates[0];
  let bestWidth = Infinity;
  for (const candidate of candidates) {
    let standing = 0;
    let inCluster = pinnedClusterCount;
    for (let i = candidate.folded; i < total; i++) {
      standing += shedding?.index === i ? shedding.widths[candidate.shed] : memberWidths[i];
      if (memberGroups[i] === 'cluster') inCluster++;
    }
    const width = (candidate.folded > 0 ? moreWidth : 0) + standing + Math.max(0, inCluster - 1) * gapPx;
    // Two zones and no gap between them: the row declares none, so every gap in
    // play is already inside `pinnedWidth`. The shared helper still owns the
    // sub-pixel fudge.
    if (computeFitsInOneRow([pinnedWidth, width], containerWidth, 0, 0)) return candidate;
    // Strictly narrower, so a tie keeps the earlier candidate. That is what
    // leaves a lone icon standing when even the ⋯ would not fit: folding it
    // swaps one box for another and costs a tap.
    if (width < bestWidth) { bestWidth = width; best = candidate; }
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
 *
 * **The shedding member is measured at every level in one pass.** The pass
 * stamps each level on the row, reads the member, and puts back the level the
 * row rendered with. No paint falls between, and the answer never depends on
 * the level drawn now, so it cannot oscillate.
 */
export function usePromptActionCollapse(
  containerRef: RefObject<HTMLElement>,
  memberKeys: readonly string[],
  memberGroups: readonly FoldGroup[],
  shedding: { key: string; levels: number } | null = null,
  gapRem = 0.5,
): PromptCollapse {
  const [collapse, setCollapse] = useState<PromptCollapse>(STANDING);
  const widths = useRef<Map<string, number>>(new Map());
  const keys = memberKeys.join(' ');
  const shedKey = shedding?.key ?? null;
  const shedLevels = shedding?.levels ?? 0;
  useLayoutEffect(() => {
    // The set changed, so a member may never have been measured. Start from a
    // fully standing row: `useLayoutEffect` runs before paint, so the pass that
    // measures it and the pass that folds it land in the same frame.
    widths.current = new Map();
    setCollapse(STANDING);
  }, [keys]);
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const levelKey = (level: number) => `${shedKey}@${level}`;
    const measureShedLevels = (seen: Map<string, number>) => {
      const member = container.querySelector<HTMLElement>(`[${FOLD_KEY_ATTR}="${shedKey}"]`);
      if (!member) return;
      const rendered = container.getAttribute(SHED_ATTR);
      for (let level = 0; level < shedLevels; level++) {
        container.setAttribute(SHED_ATTR, String(level));
        seen.set(levelKey(level), member.getBoundingClientRect().width);
      }
      if (rendered === null) container.removeAttribute(SHED_ATTR);
      else container.setAttribute(SHED_ATTR, rendered);
    };
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
      if (shedKey !== null) measureShedLevels(seen);
      const fallback = moreWidth || seen.values().next().value || 0;
      const shedIndex = shedKey === null ? -1 : memberKeys.indexOf(shedKey);
      const next = computePromptCollapse({
        containerWidth: contentWidthOf(container),
        pinnedWidth,
        memberWidths: memberKeys.map((k) => seen.get(k) ?? fallback),
        memberGroups,
        pinnedClusterCount,
        gapPx: gapRem * getRemPx(),
        moreWidth: moreWidth || fallback,
        shedding: shedIndex === -1 ? null : {
          index: shedIndex,
          widths: Array.from({ length: shedLevels }, (_, level) => seen.get(levelKey(level)) ?? fallback),
        },
      });
      setCollapse((prev) => (prev.folded === next.folded && prev.shed === next.shed ? prev : next));
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
  }, [containerRef, keys, gapRem, memberKeys, memberGroups, shedKey, shedLevels]);
  return collapse;
}
