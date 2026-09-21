/** Where a scroll container must sit for a focused field to be visible.
 *
 *  The mobile keyboard shrinks `.app-shell` through `--app-height`, so the
 *  pane's scroll container shrinks under a field the browser already scrolled
 *  into view. Nothing re-runs after that, which is what leaves a tapped field
 *  behind the keyboard. See
 *  `docs/plans/2026-09-19-a-focused-field-stays-above-the-keyboard.md`.
 *
 *  Kept apart from the hook that drives it so the geometry is unit-testable
 *  without a viewport, a keyboard or a layout engine. */

/** One field measured against one scroll container, all in viewport pixels.
 *  `viewTop` / `viewBottom` are the container's CLIENT box, so a border or a
 *  scrollbar gutter is already discounted by the caller. */
export interface RevealGeometry {
  scrollTop: number;
  viewTop: number;
  viewBottom: number;
  fieldTop: number;
  fieldBottom: number;
  /** Clear space to leave above the field. */
  marginTop: number;
  /** Clear space to leave below it. Larger than `marginTop` when something is
   *  drawn over the foot of the viewport, which the keyboard accessory bar is. */
  marginBottom: number;
}

/** Below this, the move is not worth a layout or the momentum it can cancel. */
const MIN_MOVE_PX = 1;

/** The container's new `scrollTop`, or `null` when no move is owed.
 *
 *  Both margins hold for any offset in `[bottomFit, topFit]`, so the answer is
 *  the point in that range nearest the container's current position. That is
 *  what makes a field already clear of both stay exactly where it is, and what
 *  makes repeated calls converge rather than creep.
 *
 *  The range is EMPTY for a field taller than the gap the two margins leave.
 *  The TOP wins there, because reading a field from its first line beats
 *  reading it from its last.
 *
 *  Clamped at zero, which is the domain of `scrollTop` rather than the
 *  container's range. The DOM clamps the far end on write, and half a range
 *  clamp here would only hide that from the caller. */
export function revealScrollTop(g: RevealGeometry): number | null {
  const { scrollTop, viewTop, viewBottom, fieldTop, fieldBottom, marginTop, marginBottom } = g;
  /** Where the container sits with the field's bottom exactly on its margin. */
  const bottomFit = scrollTop + fieldBottom + marginBottom - viewBottom;
  /** Where it sits with the field's top exactly on its margin. */
  const topFit = scrollTop + fieldTop - marginTop - viewTop;
  const fit = bottomFit > topFit
    ? topFit
    : Math.min(Math.max(scrollTop, bottomFit), topFit);
  const target = Math.max(0, fit);
  if (Math.abs(target - scrollTop) < MIN_MOVE_PX) return null;
  return target;
}

/** The first ancestor that can actually scroll `el` vertically, or `null`.
 *
 *  Both halves are required. An element that declares `overflow-y: auto`
 *  without overflowing scrolls nothing, and one that overflows without
 *  declaring it clips instead. Read at reveal time rather than at focus time,
 *  because a form that fits the full-height pane starts overflowing the moment
 *  the keyboard shrinks it. */
export function nearestScrollableAncestor(el: Element | null): HTMLElement | null {
  for (let node = el?.parentElement ?? null; node; node = node.parentElement) {
    if (node.scrollHeight - node.clientHeight <= MIN_MOVE_PX) continue;
    const { overflowY } = getComputedStyle(node);
    if (overflowY === 'auto' || overflowY === 'scroll') return node;
  }
  return null;
}
