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

/** Move a MOUNTED app to an app fragment, without reloading it. Apps that care
 *  listen for `hashchange`; the rest are untouched.
 *
 *  `location.replace` again, and for the reason in this file's header: a plain
 *  `location.hash = …` PUSHES a session-history entry, so a fragment delivery
 *  would extend the joint history exactly the way an app switch used to. The
 *  URL differs from the frame's only in its fragment, so the navigation stays
 *  same-document: the app is not reloaded and `hashchange` still fires.
 *
 *  A same-document navigation fires no `load`, so the caller must raise no
 *  load cover. One would sit over a live app until its fuse.
 *
 *  Returns whether the frame moved. False means there was nothing to do: a
 *  detached frame with no browsing context, or one already on that URL. The
 *  second case is what makes this idempotent. Two callers deliver a fragment,
 *  this frame's own effect and `openApp`, and idempotence is why they cannot
 *  fight over it. */
export function setAppFrameHash(iframe: HTMLIFrameElement, fragment: string): boolean {
  const win = iframe.contentWindow;
  if (!win) return false;
  // Built from the frame's OWN href, never from a bare `#frag`: `replace`
  // resolves a relative URL against the CALLER's document, so the host would
  // navigate the app frame to the host page.
  const target = new URL(win.location.href);
  target.hash = fragment;
  if (target.href === win.location.href) return false;
  win.location.replace(target.href);
  return true;
}

/** Split a frame src into the part naming the DOCUMENT and the fragment inside
 *  it. A change of document part is a navigation; a change of fragment alone is
 *  a move within the app the frame already has. */
export function splitFrameSrc(src: string): { doc: string; fragment: string } {
  const hash = src.indexOf('#');
  if (hash === -1) return { doc: src, fragment: '' };
  return { doc: src.slice(0, hash), fragment: src.slice(hash + 1) };
}
