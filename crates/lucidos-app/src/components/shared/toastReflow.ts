/** Pure part of the toast-stack reflow (FLIP) animation.
 *
 *  A new toast is prepended to the toast list (`showToast` in store.ts) and
 *  lands at the top of its pane's column-stacked, top-pinned stack
 *  (`toastColumns.ts`), so every surviving toast IN THAT COLUMN is instantly
 *  re-laid-out downward by the new toast's height + gap. Left alone that reflow
 *  is an abrupt jump. The Toast component captures each surviving toast's OLD
 *  top before the commit and its NEW top after, then plays the delta back to
 *  zero, so the survivors glide to their new positions instead of snapping.
 *  The other pane's column does not move, so its toasts yield a zero delta and
 *  are skipped by the threshold below.
 *
 *  This helper is the DOM-free core: given the previous render's ids, the
 *  captured old tops, and the current tops, it returns the per-toast shift to
 *  animate. Newly inserted toasts are excluded (their CSS `toast-in` entry
 *  animation owns them); sub-pixel movements are dropped as noise. */
export interface ToastTop {
  id: number;
  /** Viewport-relative top of the toast (getBoundingClientRect().top). */
  top: number;
}

export interface ToastShift {
  id: number;
  /** old − new: positive when the toast slid up (a toast above was removed),
   *  negative when it slid down (a toast was inserted above it). The animation
   *  starts at translateY(delta) — the OLD spot — and eases back to 0. */
  delta: number;
}

/** Sub-pixel reflows aren't worth an animation frame — treat them as no-ops. */
const MOVEMENT_THRESHOLD_PX = 1;

export function computeToastShifts(
  prevIds: ReadonlySet<number>,
  oldTops: ReadonlyMap<number, number>,
  current: readonly ToastTop[],
  threshold = MOVEMENT_THRESHOLD_PX,
): ToastShift[] {
  const shifts: ToastShift[] = [];
  for (const { id, top } of current) {
    // Newly inserted this render → its `toast-in` entry animation owns it.
    if (!prevIds.has(id)) continue;
    const oldTop = oldTops.get(id);
    if (oldTop === undefined) continue;
    const delta = oldTop - top;
    if (Math.abs(delta) < threshold) continue;
    shifts.push({ id, delta });
  }
  return shifts;
}
