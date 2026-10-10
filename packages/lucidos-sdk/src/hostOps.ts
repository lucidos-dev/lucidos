/**
 * What the host asks this frame to do, now that it cannot reach in.
 *
 * The shell used to drive these through `contentWindow`: read
 * `lucidos._capture` to screenshot an app for the agent, and `location.replace`
 * to deliver an app fragment. An opaque origin denies both, so the frame does
 * them for itself. Switching apps remounts the frame and asks nothing of it.
 *
 * No op takes a whole URL. Any page that frames the app could otherwise send
 * it anywhere, a `javascript:` URL included.
 *
 * `location.replace` and not `location.hash`, for the reason
 * `components/apps/iframeNav.ts` gives on the host side: a plain hash assignment
 * pushes a session-history entry, and on an iOS PWA the edge-swipe-back gesture
 * then replays it.
 */

import { serveHost } from './_bridge';
import { capture } from './capture';
import { serveFind } from './find';

export function installHostOps(): void {
  serveHost('capture', () => capture());

  serveHost('hash', (args) => {
    const fragment = (args as { fragment?: string })?.fragment ?? '';
    // Built from this frame's OWN href. Resolving a bare `#frag` against the
    // host's document is what would send the frame to the host page.
    const target = new URL(location.href);
    target.hash = fragment;
    if (target.href === location.href) return false;
    location.replace(target.href);
    return true;
  });

  // Find in app: the host's find bar reads no frame text, so the frame runs it.
  serveHost('find', serveFind);
}
