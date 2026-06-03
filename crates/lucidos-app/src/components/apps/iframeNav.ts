/** Navigate an iframe without recording a session-history entry.
 *
 *  Setting `iframe.src` is treated by browsers as a navigation that goes into
 *  the parent's joint session history (HTML spec; WebKit bug #9166). On iOS
 *  PWAs the edge-swipe-back gesture replays those entries and surfaces a
 *  cached snapshot of a previous app pane mid-swipe. `location.replace()`
 *  performs the navigation without extending history.
 *
 *  An in-document iframe is normally guaranteed a browsing context, but a
 *  detached iframe (mid-unmount, or one removed from the DOM between layout
 *  and effect flushes) has `contentWindow === null` and would throw a
 *  TypeError on `.location`. We return `false` instead — caller surfaces a
 *  toast and skips the `lastSrcRef` update so the next render retries the
 *  navigation against whatever iframe is mounted then.
 *
 *  No silent fallback to `iframe.src = url`: that would reintroduce the exact
 *  bug being fixed (a history entry per app switch) with no test signal. */
export function navigateAppIframe(iframe: HTMLIFrameElement, url: string): boolean {
  const win = iframe.contentWindow;
  if (!win) return false;
  win.location.replace(url);
  return true;
}
