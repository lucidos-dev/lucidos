/** User-scroll activity tracker.
 *
 *  iOS cancels an in-flight momentum scroll the instant ANY code writes the
 *  scroll container's `scrollTop` — the "scrolling randomly stops instead of
 *  going further when you let go" bug on the iOS PWA. The only programmatic
 *  scrollTop write that fires INDEPENDENTLY of the user's gesture is
 *  `forceWebKitRepaint`'s ±1px compositor-recovery nudge (streaming throttle,
 *  thread-open burst, settle probe, page-resume); ticking on a timer, it lands
 *  mid-fling and kills the momentum. So the nudge must stand down while a
 *  finger-drag or its momentum tail is in flight. During a real scroll the
 *  compositor is already committing layers (a manual scroll IS the repaint), so
 *  the nudge is redundant then anyway — suppressing it costs no recovery.
 *
 *  We key off `touchmove` because touch events are NEVER produced
 *  programmatically — unlike `scroll` events, which the nudge itself fires (a
 *  scroll-based detector would fight its own output) and which auto-tail
 *  scroll-to-bottom also fires (which must NOT read as user scrolling). A single
 *  passive, capture-phase document listener covers every scroll container; a tap
 *  (touchstart/touchend with no touchmove) never marks activity, so only real
 *  drags suppress the nudge. */

let lastTouchMoveAt = -Infinity;
let installed = false;

/** Window (ms) after the last touch-drag during which the user counts as still
 *  scrolling. Covers the drag itself plus the momentum tail after finger release
 *  — long enough to survive a normal fling's coast, short enough that a genuinely
 *  needed streaming repaint resumes within a throttle tick once motion settles. */
export const USER_SCROLL_WINDOW_MS = 1200;

function nowMs(): number {
  return typeof performance !== 'undefined' && typeof performance.now === 'function'
    ? performance.now()
    : Date.now();
}

function onTouchMove(): void {
  lastTouchMoveAt = nowMs();
}

/** Install the global touch listener once. Lazy (called from isUserScrolling) so
 *  the module has no import-time side effects; idempotent + SSR/test-safe. The
 *  first `forceWebKitRepaint` (thread-open burst) installs it well before the user
 *  can start a fling. */
function ensureInstalled(): void {
  if (installed || typeof document === 'undefined' || typeof document.addEventListener !== 'function') return;
  installed = true;
  document.addEventListener('touchmove', onTouchMove, { passive: true, capture: true });
}

/** True while a user touch-drag — or the momentum tail right after release — is
 *  likely in flight, so momentum-cancelling scrollTop writes should stand down. */
export function isUserScrolling(): boolean {
  ensureInstalled();
  return nowMs() - lastTouchMoveAt < USER_SCROLL_WINDOW_MS;
}
