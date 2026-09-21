import { APP_FRAME_ISOLATED } from './appFrameSandbox';
import { tellFrame } from '../../store/actions/app-bridge';

/** Deliver an app fragment to a MOUNTED app, and re-scope the frame's own URL.
 *
 *  A change of DOCUMENT is not here: the host keys the frame on the document,
 *  so a change of app remounts the element with the new URL as its initial src.
 *  An isolated frame denies `contentWindow.location`, and mutating `src` on a
 *  live element adds a joint-history entry (WebKit #9166). So a remount is the
 *  one move that needs nothing from the app.
 */

/** Move a MOUNTED app to an app fragment, without reloading it. Apps that care
 *  listen for `hashchange`; the rest are untouched.
 *
 *  The frame runs `location.replace`, for the reason in this file's header: a
 *  plain `location.hash = …` PUSHES a session-history entry, so a fragment
 *  delivery would extend the joint history an app switch used to. The
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
  // An isolated frame denies `contentWindow.location`, so it moves itself. The
  // SDK's `hash` handler runs this same arithmetic against its own href, the
  // idempotence check included. The answer it reaches cannot come back, so here
  // the return means the request was delivered. Both callers ignore it.
  if (APP_FRAME_ISOLATED) return tellFrame(iframe, 'hash', { fragment });
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
