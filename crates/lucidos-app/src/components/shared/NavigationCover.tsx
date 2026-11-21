import { useLayoutEffect, useRef, useState } from 'preact/hooks';
import { scaledDurationMs } from '../../utils/motion';

/** The two motions a navigation cover plays, each with its 1x length:
 *  - `arrive` clears off the arriving view (`--duration-normal`). The leaving
 *    view has already gone, since a content pane unmounts it.
 *  - `dip` rises over the leaving view, holds, and clears off the arriving one
 *    (`--duration-slow`). It needs both views mounted, and the host swaps them
 *    at its midpoint, as the drawer does.
 *
 *  The fuse is the length, scaled by the Animation speed slider, plus a fixed
 *  slack. So a cover outlives its own animation at any speed. The fuse also
 *  ends a cover under reduced motion, where the CSS drops the animation. An
 *  `animationend`-driven end would then never fire. */
export const NAV_COVER_MOTIONS = {
  arrive: { class: 'nav-cover', animMs: 200 },
  dip: { class: 'nav-cover nav-cover-dip', animMs: 300 },
} as const;
export type NavCoverMotion = keyof typeof NAV_COVER_MOTIONS;
const NAV_COVER_SLACK_MS = 50;

/** How long a cover playing `motion` stays mounted. */
export function navCoverFuseMs(motion: NavCoverMotion): number {
  return scaledDurationMs(NAV_COVER_MOTIONS[motion].animMs) + NAV_COVER_SLACK_MS;
}

/** The view that just arrived in a pane, for as long as its cover lasts, or
 *  null. `viewKey` identifies what the pane shows, and each change to a
 *  non-null key is one arrival. The first render is not a navigation, and a
 *  pane navigating to nothing has nothing arriving. */
function useArrivingView(viewKey: string | null, motion: NavCoverMotion): string | null {
  const [arriving, setArriving] = useState<string | null>(null);
  const seenKeyRef = useRef(viewKey);
  useLayoutEffect(() => {
    if (seenKeyRef.current === viewKey) return;
    seenKeyRef.current = viewKey;
    if (viewKey === null) { setArriving(null); return; }
    setArriving(viewKey);
    const fuse = setTimeout(() => setArriving(null), navCoverFuseMs(motion));
    return () => clearTimeout(fuse);
    // A host never changes its motion, so only a new view arms a fuse.
  }, [viewKey]);
  return arriving;
}

/** The navigation cover: an opaque theme surface that hides a pane's view swap
 *  (`.nav-cover`, global/host-components.css). So a view switch never shows its
 *  swap frame. Render it as the last child of the pane's positioned box, which
 *  sets its z-index.
 *
 *  A cover, not a fade on the content: a frame WebKit re-composites up from
 *  transparent is the shape of the iOS paint-loss bugs. The cover is a sibling
 *  with its own layer, and it never waits on what the view has painted.
 *
 *  A keyed element replaying a CSS animation, not a class-toggled transition.
 *  A transition needs its opaque start on screen before the clearing class
 *  lands, and silently hard-cuts when it loses that race. A fresh element's
 *  animation starts from its own first frame. Keyed on the view it covers, so a
 *  navigation arriving mid-fade restarts the animation. */
export function NavigationCover({ viewKey, motion = 'arrive' }: { viewKey: string | null; motion?: NavCoverMotion }) {
  const arriving = useArrivingView(viewKey, motion);
  return arriving === null ? null : <div key={arriving} class={NAV_COVER_MOTIONS[motion].class} aria-hidden="true" />;
}
