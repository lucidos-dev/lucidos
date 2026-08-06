import { useState, useLayoutEffect } from 'preact/hooks';
import type { RefObject } from 'preact';
import { getRemPx } from '../utils/dom';
import { computeFitsInOneRow } from './useFitsInOneRow';

/** Inputs to the content-header collapse decision. All widths in px, measured
 *  at the same moment (see the hook below). `actionWidths` is the CONTEXT
 *  actions only, ordered nearest-title first — the notifications bell is the
 *  never-collapsed anchor and travels separately as `bellWidth`. */
export interface HeaderCollapseInput {
  /** Inner width of the 3-zone flex row (`.content-header-elements`). */
  containerWidth: number;
  /** Left zone (`.panel-nav`) — flex-shrink:0, always fully visible. */
  navWidth: number;
  /** NATURAL (un-ellipsized) width of the title text, 0 when no title. */
  titleWidth: number;
  /** Context action widths, nearest-title first (bell excluded). */
  actionWidths: readonly number[];
  /** The notifications bell — always visible, never collapsed. */
  bellWidth: number;
  /** The ⋯ overflow trigger, present only while `collapsed > 0`. */
  moreWidth: number;
  /** Flex gap — between the three zones AND between adjacent action items. */
  gapPx: number;
}

export interface HeaderCollapseResult {
  /** How many LEADING (nearest-title) actions move into the ⋯ menu.
   *  Always 0 or 2..N: collapsing exactly one saves nothing — the ⋯ trigger
   *  replaces it 1:1 — so the first collapse step takes the two nearest. */
  collapsed: number;
  /** True only once the icons are at their minimal state (⋯ + bell, or no
   *  collapse possible) and the full title STILL doesn't fit — the title
   *  ellipsizes on its right edge (CSS does the clipping; this flag is the
   *  pure-math mirror for tests). */
  titleEllipsized: boolean;
}

/** Width of the right icon row for a given collapse count: the ⋯ trigger (when
 *  anything is collapsed) + the remaining visible actions + the bell, plus the
 *  uniform gap between adjacent items. Exported for the non-overlap invariant
 *  test. */
export function iconsRowWidth(
  input: Pick<HeaderCollapseInput, 'actionWidths' | 'bellWidth' | 'moreWidth' | 'gapPx'>,
  collapsed: number,
): number {
  const widths: number[] = [];
  if (collapsed > 0) widths.push(input.moreWidth);
  for (let i = collapsed; i < input.actionWidths.length; i++) widths.push(input.actionWidths[i]);
  widths.push(input.bellWidth);
  let total = 0;
  for (const w of widths) total += w;
  return total + (widths.length - 1) * input.gapPx;
}

/** Pure collapse math, exported so the decision can be unit-tested without
 *  ResizeObserver/jsdom (same pattern as `computeFitsInOneRow`). Picks the
 *  SMALLEST collapse count whose icon row leaves room for the FULL title next
 *  to the nav zone; icons always give way before the title truncates. The
 *  candidate counts are 0, then 2..N (never exactly 1 — see
 *  `HeaderCollapseResult.collapsed`). The fit test delegates to
 *  `computeFitsInOneRow` so the gap model and its sub-pixel fudge live in one
 *  place. */
export function computeHeaderCollapse(input: HeaderCollapseInput): HeaderCollapseResult {
  const n = input.actionWidths.length;
  const candidates: number[] = [0];
  for (let c = 2; c <= n; c++) candidates.push(c);

  const fits = (c: number): boolean => computeFitsInOneRow(
    [input.navWidth, input.titleWidth, iconsRowWidth(input, c)],
    input.containerWidth,
    input.gapPx,
  );

  for (const c of candidates) {
    if (fits(c)) return { collapsed: c, titleEllipsized: false };
  }
  // Even the minimal icon row (⋯ + bell) can't host the full title — the
  // title's flex zone shrinks and CSS ellipsizes it on the right.
  return { collapsed: candidates[candidates.length - 1], titleEllipsized: true };
}

/** Inputs to the MOBILE content-header collapse decision. The mobile header
 *  lays its title out as an ABSOLUTELY row-centered box (not a flex zone —
 *  see `.mobile-content-title` in mobile.css), so its geometry differs from the
 *  desktop 3-zone model above: a symmetric reserve around the row middle keeps
 *  the centered title clear of BOTH clusters, bounded by whichever cluster's
 *  inner edge is nearest the centre. Because the trailing cluster
 *  (refresh/open/fullscreen/bell) is much wider than the leading one and the
 *  viewport is a fixed device width, a constant rem reserve can't hold at every
 *  ui-scale — the widths are measured instead. All widths in px, captured at one
 *  moment; every input is independent of the current collapse count so the
 *  computation can't oscillate. */
export interface MobileHeaderCollapseInput {
  /** `.mobile-header-row` box width (its centre is the title's true axis). */
  rowWidth: number;
  /** Leading cluster (hamburger + nav slot) right edge, relative to the row's
   *  left edge — flex-shrink:0, so collapse-invariant. */
  leadingRight: number;
  /** Distance from the trailing cluster's right edge (the always-present bell,
   *  pinned right by the flex spacer) to the row's right edge — the right
   *  padding / safe-area inset, whatever its source. Collapse-invariant. */
  trailingRightGap: number;
  /** NATURAL (un-ellipsized) width of the title text, 0 when no title. */
  titleWidth: number;
  /** Context action widths, nearest-title first (bell excluded). */
  actionWidths: readonly number[];
  /** The notifications bell — always visible, never collapsed. */
  bellWidth: number;
  /** The ⋮ overflow trigger, present only while collapsed > 0. */
  moreWidth: number;
  /** Flex gap between adjacent trailing items. */
  gapPx: number;
}

export interface MobileHeaderCollapseResult {
  /** How many LEADING (nearest-title) trailing actions fold into the ⋮ menu.
   *  0 or 2..N, never exactly 1 (⋮ replaces one 1:1 — no gain), same as the
   *  desktop rule. */
  collapsed: number;
  /** Max-width (px) for the absolutely row-centered title box at the chosen
   *  collapse count. Together with `titleShift` it clears both clusters: the
   *  title renders at its natural width when shorter, and ellipsizes at this
   *  width when even the fully-collapsed row can't host it. */
  titleMaxWidth: number;
  /** Horizontal offset (px, positive = right) applied to the row-centered title
   *  box. 0 while the title fits the SYMMETRIC reserve, which is the preferred
   *  layout; otherwise the LEAST slide that pulls the box clear of both
   *  clusters, so it can spend the roomier side's slack instead of truncating
   *  into it. */
  titleShift: number;
  /** True only once the icons are minimal and the full title STILL doesn't fit
   *  the whole gap between the two clusters. */
  titleEllipsized: boolean;
}

/** Pure mobile collapse math, exported for unit testing without the DOM.
 *  Three tiers, in preference order:
 *
 *  1. **Centred on the row middle**, the layout the mobile header is built
 *     around, so collapses are spent keeping it: the FEWEST nearest-title
 *     actions folded into ⋮ whose symmetric reserve hosts the full title.
 *  2. **Slid off centre.** The symmetric reserve is bounded by whichever cluster
 *     is NEARER the centre, so it leaves the roomier side's slack unused. A
 *     settings subview carries no context actions at all, which makes the
 *     leading cluster (hamburger + back/forward) the binding one and strands
 *     the whole trailing half: the title truncated next to blank space. Rather
 *     than ellipsize into that room, slide the box by the least amount that
 *     clears both clusters and let it use the full span between them.
 *  3. **Ellipsized** at the widest span there is, which is always the
 *     fully-collapsed one (folding an action into ⋮ moves the trailing cluster
 *     right, so unlike the symmetric reserve the span never plateaus).
 *
 *  Candidate counts are 0, then 2..N (never exactly 1). */
export function computeMobileHeaderCollapse(input: MobileHeaderCollapseInput): MobileHeaderCollapseResult {
  const n = input.actionWidths.length;
  const candidates: number[] = [0];
  for (let c = 2; c <= n; c++) candidates.push(c);

  const center = input.rowWidth / 2;
  const leadingHalf = center - input.leadingRight; // room on the LEFT of centre

  /** Inner edge of the trailing cluster at collapse count `c`. `iconsRowWidth`
   *  gives its width (⋮ + remaining actions + bell + gaps), the same helper the
   *  desktop path uses. */
  const trailingLeftAt = (c: number): number =>
    input.rowWidth - input.trailingRightGap - iconsRowWidth(input, c);
  /** Symmetric centred box that clears BOTH clusters at collapse count `c`. */
  const symmetricAt = (c: number): number =>
    Math.max(0, 2 * Math.min(leadingHalf, trailingLeftAt(c) - center));
  /** The whole gap between the clusters: what an off-centre box can use. */
  const spanAt = (c: number): number => Math.max(0, trailingLeftAt(c) - input.leadingRight);
  /** Least slide (px, right-positive) that pulls a row-centred box of `painted`
   *  width inside both clusters; 0 whenever the centred box already clears
   *  them. Only called with `painted <= spanAt(c)`, so the clamp range is real. */
  const shiftAt = (c: number, painted: number): number => {
    const centredLeft = center - painted / 2;
    const left = Math.min(Math.max(centredLeft, input.leadingRight), trailingLeftAt(c) - painted);
    return left - centredLeft;
  };

  for (const c of candidates) {
    const box = symmetricAt(c);
    if (box + 0.5 >= input.titleWidth) {
      return { collapsed: c, titleMaxWidth: box, titleShift: 0, titleEllipsized: false };
    }
  }
  for (const c of candidates) {
    const span = spanAt(c);
    if (span + 0.5 >= input.titleWidth) {
      return {
        collapsed: c,
        titleMaxWidth: span,
        titleShift: shiftAt(c, Math.min(input.titleWidth, span)),
        titleEllipsized: false,
      };
    }
  }
  const last = candidates[candidates.length - 1];
  const span = spanAt(last);
  return { collapsed: last, titleMaxWidth: span, titleShift: shiftAt(last, span), titleEllipsized: true };
}

/**
 * Progressive collapse count for the content-header action icons (desktop).
 *
 * `hostRef` is the `.content-header-actions` element. There are two layouts:
 *
 * - **Desktop** — the host's enclosing `.content-header-elements` 3-zone flex
 *   row; the title is a flex zone and `computeHeaderCollapse` decides.
 * - **Mobile** — `MobileAppHeader` hosts the actions directly in
 *   `.mobile-header-row`, where the title is an ABSOLUTELY row-centered box.
 *   `computeMobileHeaderCollapse` decides the collapse count AND publishes the
 *   title box's geometry on the row (the title span, a sibling, reads both
 *   through inheritance): `--mobile-content-title-max` so it truncates before
 *   the icons instead of painting under them, and `--mobile-content-title-shift`
 *   so a title the symmetric reserve can't host slides into the roomier side's
 *   slack rather than truncating next to it. The hidden layout copy (the other
 *   layout's header is `display:none`) measures a 0-width row and no-ops to
 *   collapsed 0, dropping both overrides.
 *
 * Measurement model (all inputs independent of the current collapse state, so
 * the computation can never oscillate):
 * - container / nav widths come from the live boxes (both are invariant under
 *   collapse: the container is positioned by the split geometry, the nav is
 *   flex-shrink:0);
 * - the title's NATURAL width is its children's `scrollWidth` (an ellipsized
 *   span still reports its full content width there);
 * - icon widths are uniform: every action (and the ⋯ trigger) is a
 *   `.icon-btn.header-icon`, so the always-present bell is the measuring
 *   stick.
 *
 * Re-measures on container resize (ResizeObserver — split drags, window
 * resizes) and on title-zone mutation (MutationObserver — title text changes;
 * deliberately NOT the whole container, so notification-badge ticks don't
 * force no-op re-measures); action-set swaps re-run the effect via
 * `actionCount`. Gap comes from rem at measure time (`getRemPx`), so user
 * font scaling feeds in — no hard-coded px breakpoints.
 */
export function useHeaderActionCollapse(
  hostRef: RefObject<HTMLElement>,
  actionCount: number,
  gapRem = 0.25,
): number {
  const [collapsed, setCollapsed] = useState(0);
  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) { setCollapsed(0); return; }

    // ── Mobile branch: absolutely row-centered title, symmetric measured reserve.
    // The mobile header hosts the actions directly in `.mobile-header-row` (no
    // `.content-header-elements` wrapper), so this branch is unambiguous. ──
    const mobileRow = host.closest<HTMLElement>('.mobile-header-row');
    if (mobileRow) {
      const measure = () => {
        const rowRect = mobileRow.getBoundingClientRect();
        const rowWidth = rowRect.width;
        const bell = host.querySelector<HTMLElement>('.notifications-bell');
        // A display:none layout copy (desktop hides the mobile header and vice
        // versa) measures 0 — no collapse, and drop our override so the CSS
        // fallback governs.
        if (rowWidth <= 0 || !bell) {
          setCollapsed(0);
          mobileRow.style.removeProperty('--mobile-content-title-max');
          mobileRow.style.removeProperty('--mobile-content-title-shift');
          return;
        }
        // Icons are uniform `.icon-btn.header-icon` squares, so the always-present
        // bell is the measuring stick (matching the desktop path). Title + nav are
        // re-queried each pass — the title span mounts/unmounts with the view, and
        // its scrollWidth reports the NATURAL width regardless of the current clamp.
        const bellRect = bell.getBoundingClientRect();
        const iconWidth = bellRect.width;
        const navSlot = mobileRow.querySelector<HTMLElement>('.mobile-nav-slot');
        const titleEl = mobileRow.querySelector<HTMLElement>('.mobile-content-title');
        const { collapsed: next, titleMaxWidth, titleShift } = computeMobileHeaderCollapse({
          rowWidth,
          leadingRight: navSlot ? navSlot.getBoundingClientRect().right - rowRect.left : 0,
          trailingRightGap: rowWidth - (bellRect.right - rowRect.left),
          titleWidth: titleEl ? titleEl.scrollWidth : 0,
          actionWidths: Array.from({ length: actionCount }, () => iconWidth),
          bellWidth: iconWidth,
          moreWidth: iconWidth,
          gapPx: gapRem * getRemPx(),
        });
        setCollapsed(next);
        mobileRow.style.setProperty('--mobile-content-title-max', `${titleMaxWidth}px`);
        mobileRow.style.setProperty('--mobile-content-title-shift', `${titleShift}px`);
      };
      measure();
      const ro = new ResizeObserver(measure);
      ro.observe(mobileRow);
      // Observe childList/characterData (NOT attributes) so a title-text change
      // AND the title span's mount/unmount re-measure, while our own
      // `--mobile-content-title-max` write (an attribute mutation on the row) does
      // NOT feed back — no observer loop. Badge ticks re-measure harmlessly
      // (idempotent, collapse-independent inputs).
      const mo = new MutationObserver(measure);
      mo.observe(mobileRow, { childList: true, subtree: true, characterData: true });
      return () => {
        ro.disconnect();
        mo.disconnect();
      };
    }

    // ── Desktop branch: 3-zone flex row ──
    const container = host.closest<HTMLElement>('.content-header-elements');
    if (!container) {
      setCollapsed(0);
      return;
    }
    // The three anchor elements are structurally stable for the effect's
    // lifetime (Preact reuses the DOM nodes across re-renders; the effect
    // re-runs when the action set changes), so resolve them once instead of
    // per ResizeObserver fire — measure() runs every frame during a split
    // drag. A missing bell degrades to iconWidth 0 → "everything fits" → no
    // collapse; the flex layout still guarantees non-overlap (the title just
    // ellipsizes earlier instead of icons folding into ⋯).
    const nav = container.querySelector<HTMLElement>('.panel-nav');
    const titleZone = container.querySelector<HTMLElement>('.pane-header-content-title');
    const bell = host.querySelector<HTMLElement>('.notifications-bell');
    const measure = () => {
      const iconWidth = bell ? bell.getBoundingClientRect().width : 0;
      let titleWidth = 0;
      if (titleZone) {
        for (const child of Array.from(titleZone.children)) {
          titleWidth += (child as HTMLElement).scrollWidth;
        }
      }
      const { collapsed: next } = computeHeaderCollapse({
        containerWidth: container.clientWidth,
        navWidth: nav ? nav.getBoundingClientRect().width : 0,
        titleWidth,
        actionWidths: Array.from({ length: actionCount }, () => iconWidth),
        bellWidth: iconWidth,
        moreWidth: iconWidth,
        gapPx: gapRem * getRemPx(),
      });
      setCollapsed(next);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(container);
    // Mutations only matter where they can change an input: the title zone's
    // text (natural width). Observing the whole container would also fire on
    // every notifications-badge tick — a forced re-measure that can never
    // change the result (the badge is absolutely positioned). Action-set
    // changes re-run the effect via `actionCount`; geometry changes hit the
    // ResizeObserver.
    let mo: MutationObserver | null = null;
    if (titleZone) {
      mo = new MutationObserver(measure);
      mo.observe(titleZone, {
        childList: true,
        subtree: true,
        characterData: true,
      });
    }
    return () => {
      ro.disconnect();
      mo?.disconnect();
    };
  }, [hostRef, actionCount, gapRem]);
  // Clamp against a stale measurement from a larger action set — the layout
  // effect re-measures before paint, but the intervening render must not
  // slice past the list.
  return Math.min(collapsed, actionCount);
}
