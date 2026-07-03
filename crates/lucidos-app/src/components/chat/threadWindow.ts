/** Thread-render windowing (perf): a large focused thread used to render — and
 *  markdown-parse — every exchange synchronously on open and on each re-render,
 *  blocking the main thread for hundreds of ms (measured: ~270–500ms of pure JS,
 *  style/layout 0ms). ThreadView now renders only a contiguous TAIL of the
 *  exchange list and grows it as the user scrolls toward the top. These pure
 *  helpers own the window arithmetic so it's unit-tested independently of the
 *  component. */

/** How many trailing exchanges to render on first open of a thread. The user
 *  lands at the bottom, so only the tail is visible; older ones materialize on
 *  scroll-up. */
export const INITIAL_WINDOW = 20;

/** How many more to reveal each time the user scrolls near the top. */
export const WINDOW_STEP = 20;

/** Distance (px) from the top of the scroll container at which to grow the
 *  window — a buffer so older exchanges are ready before the user reaches the
 *  very top. */
export const WINDOW_EXPAND_MARGIN_PX = 600;

/** First index of the full exchanges array to RENDER. `renderCount` is how many
 *  trailing exchanges to show; `Infinity` (deep-link "render all") → 0. Always a
 *  contiguous tail, so the streaming/active last exchange is always included. */
export function computeRenderFromIndex(total: number, renderCount: number): number {
  if (!Number.isFinite(renderCount)) return 0;
  return Math.max(0, total - Math.max(0, Math.floor(renderCount)));
}

/** Whether more (older) exchanges remain above the current window. */
export function hasMoreAbove(total: number, renderCount: number): boolean {
  return computeRenderFromIndex(total, renderCount) > 0;
}

/** Next `renderCount` after a scroll-up expansion, capped so it never exceeds the
 *  total (keeps the value stable once everything is shown). */
export function expandRenderCount(total: number, renderCount: number): number {
  return Math.min(total, renderCount + WINDOW_STEP);
}

/** Whether a "scroll to top" must render the FULL thread before scrolling.
 *
 *  The transcript is windowed — only a trailing slice is in the DOM — so a plain
 *  smooth `scrollTo(top:0)` only reaches the top of that slice. Worse, as the
 *  smooth scroll nears the top it trips the scroll-up window-expand (see
 *  ThreadView's onScroll), which prepends a chunk and re-anchors the viewport,
 *  stalling the scroll partway. The user then had to click the chevron once per
 *  chunk to crawl to the genuine top (the "needed N clicks" bug). When older
 *  exchanges remain above the window, the chevron renders everything first (set
 *  renderCount = Infinity) and only then scrolls — landing at the true top in one
 *  action. Equivalent to hasMoreAbove, named for the scroll-to-top contract that
 *  depends on it. */
export function scrollToTopNeedsRenderAll(total: number, renderCount: number): boolean {
  return hasMoreAbove(total, renderCount);
}
