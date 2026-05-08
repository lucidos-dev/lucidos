/** Navigate an iframe without recording a session-history entry.
 *
 *  Setting `iframe.src` is treated by browsers as a navigation that goes into
 *  the parent's joint session history (HTML spec; WebKit bug #9166). On iOS
 *  PWAs the edge-swipe-back gesture replays those entries and surfaces a
 *  cached snapshot of a previous app pane mid-swipe. `location.replace()`
 *  performs the navigation without extending history.
 *
 *  No contentWindow=null fallback: an in-document iframe is guaranteed a
 *  browsing context, and a silent fallback to `iframe.src = url` would
 *  reintroduce the exact bug being fixed (a history entry per app switch)
 *  with no test signal. */
export function navigateAppIframe(iframe: HTMLIFrameElement, url: string): void {
  iframe.contentWindow!.location.replace(url);
}
