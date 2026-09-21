/**
 * What the host asks this frame to do, now that it cannot reach in.
 *
 * Three things the shell used to drive through `contentWindow`: read
 * `lucidos._capture` to screenshot an app for the agent, and `location.replace`
 * to switch apps and to deliver an app fragment. An opaque origin denies all
 * three, so the frame does them for itself.
 *
 * `location.replace` and not `location.hash`, for the reason
 * `components/apps/iframeNav.ts` gives on the host side: a plain hash assignment
 * pushes a session-history entry, and on an iOS PWA the edge-swipe-back gesture
 * then replays it.
 */

import { serveHost } from './_bridge';
import { capture } from './capture';

export function installHostOps(): void {
  serveHost('capture', () => capture());

  serveHost('navigate', (args) => {
    const url = (args as { url?: string })?.url;
    if (typeof url !== 'string' || !url) throw new Error('navigate needs a url');
    location.replace(url);
    return true;
  });

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
}
