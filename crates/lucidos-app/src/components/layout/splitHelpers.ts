import { splitRatio, scaledDurationMs, SPLIT_RATIO_KEY } from '../../store/store';
import type { SplitBounds } from '../../store/paneMinimums';

export const DEFAULT_SPLIT_RATIO = 0.4;

/* The pane minimums themselves live in `store/paneMinimums.ts`: they are
   derived from the root font size, so reading them is a DOM read, and every
   helper in this module is pure by contract. Callers measure, these compute. */

/** Mirror of var(--duration-slow) in styles/global/base.css — the duration
 *  every pane/header geometry transition runs at, AT 1x. TS timers that must
 *  outlive those transitions derive from this; update the two together.
 *
 *  1x, because the token is that literal times var(--duration-scale) (the
 *  Animation speed slider). A timer mirroring it therefore passes this through
 *  `scaledDurationMs` rather than using it raw, or the two come apart the
 *  moment the slider leaves centre. */
export const PANE_TRANSITION_MS = 300;

/** While a divider drag is live. CSS keys off this to disable the header /
 *  drawer geometry transitions so all three panels and their header regions
 *  track the pointer 1:1 instead of easing 300ms behind it. */
const RESIZING_ATTR = 'data-pane-resizing';

/** Clamp a divider position to a range that may be EMPTY, which is the whole
 *  reason this is a named function rather than a `Math.min(Math.max(...))`.
 *
 *  `lo` is what the leading pane needs and `hi` what is left after the trailing
 *  pane takes its own minimum, so `hi < lo` means the container cannot hold both
 *  at once. A bare min-then-max would silently answer `hi` there, handing the
 *  space to the trailing pane; this keeps the LEADING pane whole instead and
 *  lets the trailing one take what remains. Deterministic either way, which a
 *  drag needs and the old release-time snap never had to decide, since a free
 *  drop was legal by definition.
 *
 *  Reachable in a real configuration, not just in theory: the three pane floors
 *  are derived from the root font size and stop summing under a 1280px screen
 *  somewhere past 150% ui-scale (see store/paneMinimums.ts). */
export function clampToRange(value: number, lo: number, hi: number): number {
  if (hi < lo) return lo;
  return Math.min(Math.max(value, lo), hi);
}

/** Where a dragged split divider lands: the pointer, clamped so neither pane
 *  goes below its minimum. Returns a ratio strictly INSIDE (0, 1).
 *
 *  A clamp, NOT a free drop plus a release-time correction. The divider stops at
 *  the wall while the pointer keeps going, so the width the user releases is the
 *  width that persists.
 *
 *  Strictly inside is the load-bearing half, and it is guaranteed here rather
 *  than derived: ADR 0056's whole argument for allowing a mid-drag clamp is that
 *  a collapse-state flip is UNREACHABLE during a drag, and those attributes flip
 *  at a ratio of exactly 0 or 1. The pane-minimum clamp alone does not give that.
 *  When the container cannot hold both minimums, `clampToRange` hands back the
 *  leading pane's minimum, which can exceed the container: a drawer width
 *  persisted from a wider window can leave the split at 520px against a 525px
 *  thread minimum at 175% ui-scale, and 525/520 rounds down to exactly 1, which
 *  collapses the content pane under the pointer. The final 1px inset is what the
 *  drag handlers used to carry by hand and is the reason it existed. */
export function clampSplitRatio(pointerPx: number, totalPx: number, bounds: SplitBounds): number {
  if (totalPx <= 2) return DEFAULT_SPLIT_RATIO;
  const threadPx = clampToRange(pointerPx, bounds.minThreadPx, totalPx - bounds.minContentPx);
  return clampToRange(threadPx, 1, totalPx - 1) / totalPx;
}

/** The ratio a PERSISTED one has to migrate to at mount. Null when it is
 *  already legal, and must be left exactly where the user put it.
 *
 *  A stored ratio is a fraction while the floors are px. One legal in the
 *  window it was saved in can be illegal in this one. Raising a floor does the
 *  same to every stored ratio between the old value and the new. Nothing else
 *  re-clamps: `splitRatio` is read straight out of localStorage (`store.ts`),
 *  and only a drag or a keyboard step consults the bounds. So a user upgrades
 *  INTO whatever layout they last left, floors or not. That is how a header
 *  overlap survives the change that fixed it.
 *
 *  A MIGRATION, not a resize policy. It runs once, when the split first has a
 *  width, and it corrects only a ratio the clamp would already have refused.
 *  Re-clamping on every container resize is a different decision, and a larger
 *  one: it would hold the Conversation floor by squeezing the Canvas pane under
 *  its own, on every window the two no longer fit.
 *
 *  Null rather than the unchanged ratio, so the caller writes (and persists)
 *  nothing in the ordinary case. */
export function migratedSplitRatio(
  ratio: number, totalPx: number, bounds: SplitBounds,
): number | null {
  // No layout yet, so no width to clamp against. `clampSplitRatio` answers the
  // DEFAULT ratio here, which as a migration would be a silent reset.
  if (totalPx <= 2) return null;
  if (!Number.isFinite(ratio)) return DEFAULT_SPLIT_RATIO;
  // A collapsed pane is a settled state the user chose, not an illegal width.
  if (ratio <= 0 || ratio >= 1) return null;
  const next = clampSplitRatio(ratio * totalPx, totalPx, bounds);
  return Math.abs(next - ratio) < 1e-6 ? null : next;
}

/** Next ratio for a thread-pane visibility toggle: collapsed (0) restores the
 *  default split, anything else collapses. Shared by the toggleThreadPane
 *  intent and the content-side header double-click. */
export function toggleThreadPaneRatio(ratio: number): number {
  return ratio === 0 ? DEFAULT_SPLIT_RATIO : 0;
}

/** Mirror for the content pane: collapsed (>= 1) restores, else collapses. */
export function toggleContentPaneRatio(ratio: number): number {
  return ratio >= 1 ? DEFAULT_SPLIT_RATIO : 1;
}

/** Pixels a keyboard resize (the Narrow/Widen pane shortcuts) moves a divider
 *  per press. */
export const KEYBOARD_RESIZE_STEP_PX = 80;

/** Where a keyboard step lands the split divider, or null for a no-op. Same
 *  clamp as a drag (`clampSplitRatio`), which is the point: one wall, whichever
 *  way the user moves the divider. It never collapses a pane, and stepping back
 *  into a fully-collapsed one reopens it at its minimum width.
 *
 *  Keeps its own no-op cases, which a drag has no equivalent of: a collapsed
 *  pane is a settled state, so a step pushing further out stays put rather than
 *  re-expanding, and a container too narrow for both minimums answers null
 *  instead of `clampToRange`'s pick, since there is no gesture in flight that
 *  has to land somewhere. */
export function computeStepRatio(
  ratio: number, totalPx: number, deltaPx: number, bounds: SplitBounds,
): number | null {
  if (totalPx <= 0) return null;
  if (deltaPx > 0 && ratio >= 1) return null;
  if (deltaPx < 0 && ratio <= 0) return null;
  if (totalPx - bounds.minContentPx < bounds.minThreadPx) return null;
  const nextRatio = clampSplitRatio(ratio * totalPx + deltaPx, totalPx, bounds);
  return nextRatio === ratio ? null : nextRatio;
}

/** Where a keyboard step lands the thread drawer width, or null for a no-op.
 *  `minPx` is the drawer's floor and `maxPx` the widest it may go: the caller
 *  measures both (the floor is derived from the root font size and the desktop
 *  build, the ceiling from the row less the visible split panes' minimums), so
 *  this stays pure like every other computation in this module. Clamps
 *  immediately and never closes the drawer (that's the toggle's job). */
export function computeDrawerStepWidth(
  width: number, deltaPx: number, minPx: number, maxPx: number,
): number | null {
  if (maxPx < minPx) return null;
  const next = Math.min(Math.max(width + deltaPx, minPx), maxPx);
  return next === width ? null : next;
}

/** Call on divider pointerdown: switch the layout into 1:1 pointer-tracking
 *  mode. Nothing else to do now that a drag is clamped rather than corrected
 *  afterwards, so there is no pending correction to flush. */
export function beginPaneResize(): void {
  document.documentElement.setAttribute(RESIZING_ATTR, '');
}

/** Call on divider release: transitions come back on. The layout is already
 *  where it belongs, since the drag could not leave it anywhere else. */
export function endPaneResize(): void {
  document.documentElement.removeAttribute(RESIZING_ATTR);
}

let paneAnimateTimer: ReturnType<typeof setTimeout> | null = null;

/** Add a brief CSS transition for an explicit ratio change: a pane toggle, a
 *  maximize, a keyboard step, a layout reset. NOT for a drag, which tracks the
 *  pointer 1:1 and turns these transitions off (RESIZING_ATTR above).
 *
 *  Named for the animation rather than for the deferred snap it used to serve:
 *  a drag is clamped now and corrects nothing on release, so "snap" described
 *  no caller left. The timeout outlives the PANE_TRANSITION_MS transition so the
 *  class never drops mid-flight, and rapid repeated calls (a held keyboard-resize
 *  chord) reset the timer instead of stacking removals, since an earlier call's
 *  removal must not fire mid-way through the latest call's transition.
 *
 *  The transition scales with the Animation speed slider, so the timer does too.
 *  The 100ms is slack, not animation, and stays OUTSIDE the scaled term: at 0.1x
 *  an unscaled timer would strip `pane-animate` a tenth of the way in and snap a
 *  maximizing pane the rest of the way to full width. */
function triggerPaneAnimate() {
  const container = document.querySelector('.split-layout') as HTMLElement | null;
  if (!container) return;
  container.classList.add('pane-animate');
  if (paneAnimateTimer) clearTimeout(paneAnimateTimer);
  paneAnimateTimer = setTimeout(() => {
    paneAnimateTimer = null;
    container.classList.remove('pane-animate');
  }, scaledDurationMs(PANE_TRANSITION_MS) + 100);
}

/** Update splitRatio with animations and persist to localStorage */
export function setSplitRatio(newRatio: number) {
  triggerPaneAnimate();
  splitRatio.value = newRatio;
  localStorage.setItem(SPLIT_RATIO_KEY, String(newRatio));
}
