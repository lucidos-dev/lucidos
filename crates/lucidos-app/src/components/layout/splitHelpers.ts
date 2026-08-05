import { splitRatio, MIN_DRAWER_WIDTH, SPLIT_RATIO_KEY } from '../../store/store';

export const DEFAULT_SPLIT_RATIO = 0.4;

/** Minimum usable pane widths. A divider drag can land anywhere; on release
 *  a pane narrower than its minimum snaps to exactly that width — or
 *  collapses entirely when below half of it. The content pane's minimum is
 *  higher: its header (panel-nav + title + action icons ≈ 310px worst case)
 *  and typical content need more room than a chat column. */
export const MIN_THREAD_PANE_PX = 300;
export const MIN_CONTENT_PANE_PX = 360;

/** Mirror of var(--duration-slow) in styles/global/base.css — the duration
 *  every pane/header geometry transition runs at. TS timers that must
 *  outlive those transitions derive from this; update the two together. */
export const PANE_TRANSITION_MS = 300;

/** Pause between releasing a divider and the snap-to-minimum/hidden
 *  animation, so the layout visibly settles where the user dropped it
 *  before correcting itself. */
const SNAP_DELAY_MS = 400;

/** While a divider drag is live. CSS keys off this to disable the header /
 *  drawer geometry transitions so all three panels and their header regions
 *  track the pointer 1:1 instead of easing 300ms behind it. */
const RESIZING_ATTR = 'data-pane-resizing';

let snapTimer: ReturnType<typeof setTimeout> | null = null;
let pendingSnap: (() => void) | null = null;

/** Where a released split divider should snap to, or null to stay put.
 *  Sides are checked thread-pane first; on a window too narrow to fit two
 *  minimum panes the side the user squeezed wins and the other stays small. */
export function computeSnapRatio(ratio: number, totalPx: number): number | null {
  if (totalPx <= 0) return null;
  const threadPx = ratio * totalPx;
  const contentPx = totalPx - threadPx;
  if (threadPx <= 0 || contentPx <= 0) return null; // already fully collapsed
  if (threadPx < MIN_THREAD_PANE_PX / 2) return 0;
  // Clamp: a container narrower than one minimum pane (drawer dragged very
  // wide) would otherwise produce a ratio > 1 — out of the [0, 1] domain the
  // layout persists and renders from.
  if (threadPx < MIN_THREAD_PANE_PX) return Math.min(MIN_THREAD_PANE_PX / totalPx, 1);
  if (contentPx < MIN_CONTENT_PANE_PX / 2) return 1;
  if (contentPx < MIN_CONTENT_PANE_PX) return Math.max((totalPx - MIN_CONTENT_PANE_PX) / totalPx, 0);
  return null;
}

/** Where a released thread drawer should settle: hidden, its minimum width,
 *  or null to stay where the user dropped it. */
export function computeDrawerSnap(width: number, minWidth: number): 'close' | 'min' | null {
  if (width >= minWidth) return null;
  return width < minWidth / 2 ? 'close' : 'min';
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

/** Where a keyboard step lands the split divider, or null for a no-op.
 *  Unlike the deferred snap after a drag, a step clamps immediately: it never
 *  collapses a pane and never leaves one below its minimum — collapse belongs
 *  to the pane toggles. Stepping back into a fully-collapsed pane reopens it
 *  at its minimum width (the clamp floor/ceiling). */
export function computeStepRatio(ratio: number, totalPx: number, deltaPx: number): number | null {
  if (totalPx <= 0) return null;
  // A collapsed pane is a settled state: a step pushing further out stays put.
  if (deltaPx > 0 && ratio >= 1) return null;
  if (deltaPx < 0 && ratio <= 0) return null;
  const floor = MIN_THREAD_PANE_PX;
  const ceil = totalPx - MIN_CONTENT_PANE_PX;
  if (ceil < floor) return null; // container can't fit both minimums
  const threadPx = Math.min(Math.max(ratio * totalPx + deltaPx, floor), ceil);
  const nextRatio = threadPx / totalPx;
  return nextRatio === ratio ? null : nextRatio;
}

/** Where a keyboard step lands the thread drawer width, or null for a no-op.
 *  `maxPx` is the widest the drawer may go — the caller measures the row and
 *  reserves the visible split panes' minimums. Clamps immediately and never
 *  closes the drawer (that's the toggle's job). */
export function computeDrawerStepWidth(width: number, deltaPx: number, maxPx: number): number | null {
  if (maxPx < MIN_DRAWER_WIDTH) return null;
  const next = Math.min(Math.max(width + deltaPx, MIN_DRAWER_WIDTH), maxPx);
  return next === width ? null : next;
}

/** Call on divider pointerdown. Switches the layout into 1:1
 *  pointer-tracking mode. A snap still pending from a previous release is
 *  applied NOW rather than discarded — a below-minimum pane must never
 *  outlive the gesture that created it. The flush runs after the resizing
 *  attribute is set, so it lands instantly instead of animating under the
 *  new drag. */
export function beginPaneResize(): void {
  const flush = pendingSnap;
  cancelPendingSnap();
  document.documentElement.setAttribute(RESIZING_ATTR, '');
  flush?.();
}

/** Call on divider release. Transitions come back on immediately; `snap`
 *  (if given) runs after SNAP_DELAY_MS so the correction animates as a
 *  distinct settling step rather than fighting the drop. */
export function endPaneResize(snap?: () => void): void {
  document.documentElement.removeAttribute(RESIZING_ATTR);
  if (!snap) return;
  pendingSnap = snap;
  snapTimer = setTimeout(() => {
    snapTimer = null;
    pendingSnap = null;
    snap();
  }, SNAP_DELAY_MS);
}

/** Drop any scheduled snap and leave the layout where it is. Safe to call
 *  when nothing is pending. For explicit layout overrides (setSplitRatio)
 *  the discard is correct — the override supersedes the correction. */
export function cancelPendingSnap(): void {
  pendingSnap = null;
  if (snapTimer) { clearTimeout(snapTimer); snapTimer = null; }
}

let snapAnimateTimer: ReturnType<typeof setTimeout> | null = null;

/** Add a brief CSS transition for snap animations. The timeout outlives the
 *  PANE_TRANSITION_MS transition so the class never drops mid-flight. Rapid
 *  repeated calls (a held keyboard-resize chord) reset the timer instead of
 *  stacking removals — an earlier call's removal must not fire mid-way
 *  through the latest call's transition and cut it short. */
function triggerSnapAnimate() {
  const container = document.querySelector('.split-layout') as HTMLElement | null;
  if (!container) return;
  container.classList.add('snap-animate');
  if (snapAnimateTimer) clearTimeout(snapAnimateTimer);
  snapAnimateTimer = setTimeout(() => {
    snapAnimateTimer = null;
    container.classList.remove('snap-animate');
  }, PANE_TRANSITION_MS + 100);
}

/** Update splitRatio with animations and persist to localStorage */
export function setSplitRatio(newRatio: number) {
  cancelPendingSnap();
  triggerSnapAnimate();
  splitRatio.value = newRatio;
  localStorage.setItem(SPLIT_RATIO_KEY, String(newRatio));
}
