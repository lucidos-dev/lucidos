/** Delays (ms) for the thread-OPEN repaint burst: an immediate toggle plus
 *  setTimeout-spaced retries. setTimeout rather than chained rAFs is
 *  load-bearing, because it survives the frame coalescing that can swallow a
 *  one-shot toggle on a cold open. The span also covers a layer that blanks a
 *  beat AFTER the initial paint.
 *
 *  The tail reaches 1000ms because a long-lived iOS PWA session degrades:
 *  WKWebView blanks an already-loaded thread's layer noticeably later than a
 *  cold open does. Each attempt is a full supersede-safe toggle, so they never
 *  accumulate and cost nothing on a healthy layer.
 *
 *  Its own module because the landed-content repaint in `pageResume.ts` shares
 *  the schedule, and importing `webkitRepaint.ts` for it puts that module in
 *  the entry chunk. */
export const OPEN_REPAINT_BURST_DELAYS_MS = [0, 100, 300, 600, 1000];
