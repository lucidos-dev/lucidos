import { useState, useLayoutEffect } from 'preact/hooks';
import type { RefObject } from 'preact';
import { getRemPx } from '../utils/dom';
import { computeFitsInOneRow } from './useFitsInOneRow';

/** Inputs to a header cluster's collapse decision. All widths in px, measured
 *  at the same moment (see the hook below). The row is modelled as three zones,
 *  leading | centre | actions, which is the shape BOTH collapsing header rows
 *  have: the content pane's (hamburger | title | context actions + bell) and
 *  the thread pane's (drawer toggle | mark + workspace name | thread actions).
 *  `actionWidths` is ordered nearest-centre first, since that is the end
 *  collapse eats from. */
export interface HeaderCollapseInput {
  /** Inner width of the row. */
  containerWidth: number;
  /** Leading zone. The centre box is CENTRED on the row, so this is half of
   *  what that box leaves rather than any measured control: the same room is
   *  spent on the other side whether anything is standing there or not, and
   *  feeding that half in turns the linear fit below into exactly that
   *  symmetric rule (see the hook). */
  leadingWidth: number;
  /** Width of the centred box, 0 when the row has none. */
  centreWidth: number;
  /** Action widths, nearest-centre first (the anchor excluded). */
  actionWidths: readonly number[];
  /** A trailing member that is always visible and never collapses (the
   *  notifications bell). Zero where the cluster has none, and then it costs no
   *  gap either. */
  anchorWidth: number;
  /** The ⋯ overflow trigger, present only while `collapsed > 0`. */
  moreWidth: number;
  /** Flex gap — between the three zones AND between adjacent action items. */
  gapPx: number;
  /** Fold the WHOLE set into the ⋯ menu once it reaches this many actions, at
   *  any width. A judgment about the row rather than about the room in it: past
   *  a couple of context icons the cluster stops reading as "what I can do here"
   *  and starts reading as a toolbar, and the ⋯ menu names each one in words
   *  where the row only has glyphs. Absent, only room decides. */
  alwaysCollapseFrom?: number;
}

export interface HeaderCollapseResult {
  /** How many LEADING (nearest-title) actions move into the ⋯ menu.
   *  Always 0 or 2..N: collapsing exactly one saves nothing — the ⋯ trigger
   *  replaces it 1:1 — so the first collapse step takes the two nearest. */
  collapsed: number;
  /** True only once the icons are at their minimal state (⋯ + anchor, or no
   *  collapse possible) and the centre box STILL does not fit beside them, so
   *  the row is into the regime where its clamp has bottomed out on its own
   *  min-span and the two can reach each other (CSS clips; this flag is the
   *  pure-math mirror for tests). */
  titleEllipsized: boolean;
}

/** Width of the right icon row for a given collapse count: the ⋯ trigger (when
 *  anything is collapsed) + the remaining visible actions + the bell, plus the
 *  uniform gap between adjacent items. Exported for the non-overlap invariant
 *  test. */
export function iconsRowWidth(
  input: Pick<HeaderCollapseInput, 'actionWidths' | 'anchorWidth' | 'moreWidth' | 'gapPx'>,
  collapsed: number,
): number {
  const widths: number[] = [];
  if (collapsed > 0) widths.push(input.moreWidth);
  for (let i = collapsed; i < input.actionWidths.length; i++) widths.push(input.actionWidths[i]);
  // A cluster with no permanent anchor pays for no anchor and no gap to it.
  if (input.anchorWidth > 0) widths.push(input.anchorWidth);
  if (widths.length === 0) return 0;
  let total = 0;
  for (const w of widths) total += w;
  return total + (widths.length - 1) * input.gapPx;
}

/** Pure collapse math, exported so the decision can be unit-tested without
 *  ResizeObserver/jsdom (same pattern as `computeFitsInOneRow`). Picks the
 *  SMALLEST collapse count whose icon row fits in the room the centred box
 *  leaves at its end of the row; icons always give way before they reach the
 *  box. The candidate counts are 0, then 2..N (never exactly 1: see
 *  `HeaderCollapseResult.collapsed`). The fit test delegates to
 *  `computeFitsInOneRow` so the gap model and its sub-pixel fudge live in one
 *  place. */
export function computeHeaderCollapse(input: HeaderCollapseInput): HeaderCollapseResult {
  const n = input.actionWidths.length;
  // A set this large is folded whole, however much room there is: the only
  // question left is whether the centre zone then fits.
  const candidates: number[] = input.alwaysCollapseFrom !== undefined && n >= input.alwaysCollapseFrom
    ? [n]
    : [0];
  if (candidates[0] === 0) for (let c = 2; c <= n; c++) candidates.push(c);

  const fits = (c: number): boolean => computeFitsInOneRow(
    [input.leadingWidth, input.centreWidth, iconsRowWidth(input, c)],
    input.containerWidth,
    input.gapPx,
  );

  for (const c of candidates) {
    if (fits(c)) return { collapsed: c, titleEllipsized: false };
  }
  // Even the minimal icon row (⋯ + the anchor) does not fit beside the centred
  // box, so the two are into each other and CSS clips whichever overlaps.
  return { collapsed: candidates[candidates.length - 1], titleEllipsized: true };
}

/** Which boxes a cluster's collapse measurement reads. Every collapsing header
 *  row has the same shape, so the two callers differ only in selectors.
 *
 *  `centre` is the row's CENTRED box (the content pane's title cluster, the
 *  thread pane's brand label), measured at its own rendered width. That width
 *  is a pure function of the row's, both being a `clamp` against it, so it can
 *  be read back without feeding the collapse count into its own input.
 *
 *  It used to be measured by summing the box's CHILDREN's `scrollWidth`, the
 *  natural un-ellipsized content width, capped at the box's declared
 *  `max-width`. That is the wrong question once the box is a fixed span: what
 *  the actions have to clear is the BOX, and a short title inside a box at its
 *  min-span floor reported a fraction of it, leaving the cluster free to reach
 *  the forward chevron. */
export interface HeaderCollapseTargets {
  /** The row the cluster lives in. */
  container: string;
  /** The centred box (see above). */
  centre: string;
  /** A trailing member of the cluster that never collapses: the content pane's
   *  notifications bell, the thread pane's recovery spinner. Measured at its own
   *  width rather than assumed to be an icon button, because the two are not the
   *  same size, and re-queried on every measurement, because one of them mounts
   *  and unmounts while the header is up. */
  anchor?: string;
}

/**
 * Progressive collapse count for a header cluster's action icons.
 *
 * `hostRef` is the element holding the actions; `layout` says which copy of the
 * header this is, since both are mounted at once and only one is visible. The
 * two layouts answer the question completely differently:
 *
 * - **Mobile** collapses EVERYTHING, always. The trailing cluster is the
 *   overflow trigger plus the bell whatever the view carries, so it is a
 *   constant width, which is what lets the centred nav cluster be pinned to a
 *   fixed span in CSS (`.header-nav-cluster`) and land in the same place on the
 *   thread and content panes. Nothing is measured. This branch used to run a
 *   `computeMobileHeaderCollapse` that folded the nearest-title actions in one
 *   at a time and published `--mobile-content-title-max` / `-shift` so the
 *   centred box could slide into whichever side had slack; a constant trailing
 *   cluster makes all of it dead weight, and the CSS bounds the title now.
 * - **Desktop** keeps the measured progressive collapse, with
 *   `computeHeaderCollapse` picking the fewest collapses that fit the cluster
 *   into the room the centred box leaves at its end of the row.
 *
 * Desktop measurement model (all inputs independent of the current collapse
 * state, so the computation can never oscillate):
 * - the container's width comes from the live box, and is set by the split
 *   geometry rather than by anything in the row;
 * - so is the centre box's, which is a `clamp` against the container, and the
 *   leading width is derived as half of what it leaves (the box is centred, so
 *   that same room is spent on the other side whether anything stands there or
 *   not);
 * - icon widths are uniform: every action (and the ⋯ trigger) is a
 *   `.icon-btn.header-icon`, so any one of them is the measuring stick.
 *
 * Re-measures on a resize of EITHER box (ResizeObserver: split drags, window
 * resizes, and a retuned span token, which the live style remote can write
 * without the container changing at all); action-set swaps re-run the effect
 * via `actionCount`. Gap comes from rem at measure time (`getRemPx`), so user
 * font scaling feeds in, with no hard-coded px breakpoints. Nothing watches the
 * title TEXT any more: it cannot move the box's edges.
 */
export function useHeaderActionCollapse(
  hostRef: RefObject<HTMLElement>,
  actionCount: number,
  layout: 'desktop' | 'mobile',
  targets: HeaderCollapseTargets,
  opts: {
    /** Flex gap between adjacent items, in rem. */
    gapRem?: number;
    /** See `HeaderCollapseInput.alwaysCollapseFrom`. */
    alwaysCollapseFrom?: number;
  } = {},
): number {
  const { gapRem = 0.25, alwaysCollapseFrom } = opts;
  const [collapsed, setCollapsed] = useState(0);
  useLayoutEffect(() => {
    // Mobile has nothing to measure: the answer is "all of them", below.
    if (layout === 'mobile') return;

    const host = hostRef.current;
    if (!host) { setCollapsed(0); return; }

    const container = host.closest<HTMLElement>(targets.container);
    if (!container) {
      setCollapsed(0);
      return;
    }
    // The two observed elements are structurally stable for the effect's
    // lifetime (Preact reuses the DOM nodes across re-renders; the effect
    // re-runs when the action set changes), so resolve them once instead of
    // per ResizeObserver fire — measure() runs every frame during a split
    // drag. A missing bell degrades to iconWidth 0 → "everything fits" → no
    // collapse; the centred box's own clamp still keeps it off the cluster
    // everywhere but that clamp's min-span arm.
    const titleZone = container.querySelector<HTMLElement>(targets.centre);
    const measure = () => {
      // The stick is whichever control the host currently renders: every action
      // and the ⋯ trigger are the same `.icon-btn.header-icon` box, so any of
      // them measures all of them. The anchor is NOT one (the recovery spinner
      // is a bare glyph), so it carries its own width.
      const stick = host.querySelector<HTMLElement>('.icon-btn');
      const iconWidth = stick ? stick.getBoundingClientRect().width : 0;
      // Re-queried per measurement rather than resolved once: the thread pane's
      // anchor appears only while sessions are resuming, and a cluster that
      // grew a member without re-measuring is exactly how it ends up crowding
      // the centred brand.
      const anchor = targets.anchor ? host.querySelector<HTMLElement>(targets.anchor) : null;
      // The box's own width, not its content's: it is a fixed span with its
      // members at the ends, so what the cluster has to clear is the box.
      const titleWidth = titleZone ? titleZone.getBoundingClientRect().width : 0;
      const containerWidth = container.clientWidth;
      const { collapsed: next } = computeHeaderCollapse({
        containerWidth,
        leadingWidth: Math.max(0, (containerWidth - titleWidth) / 2),
        centreWidth: titleWidth,
        actionWidths: Array.from({ length: actionCount }, () => iconWidth),
        anchorWidth: anchor ? anchor.getBoundingClientRect().width : 0,
        moreWidth: iconWidth,
        gapPx: gapRem * getRemPx(),
        alwaysCollapseFrom,
      });
      setCollapsed(next);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(container);
    // The centred box too, and not because the container's resize misses a
    // split drag (it does not): its width is `clamp(min, 100% - 2 * reserve,
    // span)`, and the live style remote can rewrite any of those three tokens
    // on a running app, which moves the box's edges with the row standing
    // still. Observing the box is also what lets the title TEXT go unwatched:
    // a MutationObserver used to re-measure on every title change, back when
    // the input was the content's natural width rather than the box's.
    if (titleZone) ro.observe(titleZone);
    // The host's own membership, which changes without the action set changing:
    // an anchor that mounts (the recovery spinner) is a wider cluster nothing
    // else would report. `childList` WITHOUT `subtree`, which is what keeps the
    // notifications badge's ticks out of it: the badge lives inside the bell.
    // Re-measuring after a collapse cannot loop, since every input above is
    // independent of the collapse count.
    const hostMo = new MutationObserver(measure);
    hostMo.observe(host, { childList: true });
    return () => {
      ro.disconnect();
      hostMo.disconnect();
    };
  }, [hostRef, actionCount, gapRem, alwaysCollapseFrom, layout, targets.container,
      targets.centre, targets.anchor]);
  // Mobile collapses the whole set, unconditionally and without a measurement,
  // so it is also immune to the stale-count race the desktop clamp handles.
  if (layout === 'mobile') return actionCount;
  // Clamp against a stale measurement from a larger action set: the layout
  // effect re-measures before paint, but the intervening render must not
  // slice past the list.
  return Math.min(collapsed, actionCount);
}
