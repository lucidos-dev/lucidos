import { useLayoutEffect, useRef, useState } from 'preact/hooks';
import { scaledDurationMs } from '../../utils/motion';

/** The arrival animations at 1x (`--duration-normal`). The fuse is this,
 *  scaled by the Animation speed slider, plus a fixed slack. So an arrival
 *  outlives its own fade at any speed. The fuse also ends an arrival under
 *  reduced motion, where the CSS drops the animation. An `animationend`-driven
 *  end would then never fire. */
const NAV_COVER_ANIM_MS = 200;
const NAV_COVER_SLACK_MS = 50;

/** The view that just arrived in a pane, for as long as its arrival fades, or
 *  null. `viewKey` identifies what the pane shows, and each change to a
 *  non-null key is one arrival. The first render is not a navigation, and a
 *  pane navigating to nothing has nothing arriving. */
export function useArrivingView(viewKey: string | null): string | null {
  const [arriving, setArriving] = useState<string | null>(null);
  const seenKeyRef = useRef(viewKey);
  useLayoutEffect(() => {
    if (seenKeyRef.current === viewKey) return;
    seenKeyRef.current = viewKey;
    if (viewKey === null) { setArriving(null); return; }
    setArriving(viewKey);
    const fuse = setTimeout(() => setArriving(null), scaledDurationMs(NAV_COVER_ANIM_MS) + NAV_COVER_SLACK_MS);
    return () => clearTimeout(fuse);
  }, [viewKey]);
  return arriving;
}

/** The navigation cover: every arriving view of a pane fades in from behind an
 *  opaque theme surface (`.nav-cover`, global/host-components.css). So a view
 *  switch never shows its swap frame. Render it as the last child of the pane's
 *  positioned box, which sets its z-index.
 *
 *  A cover, not a fade on the content: a frame WebKit re-composites up from
 *  transparent is the shape of the iOS paint-loss bugs. The cover is a sibling
 *  with its own layer, and it never waits on what the view has painted.
 *
 *  A keyed element replaying a CSS animation, not a class-toggled transition.
 *  A transition needs its opaque start on screen before the clearing class
 *  lands, and silently hard-cuts when it loses that race. A fresh element's
 *  animation starts from its own first frame. Keyed on the view it covers, so a
 *  navigation arriving mid-fade restarts from opaque. */
export function NavigationCover({ viewKey }: { viewKey: string | null }) {
  const arriving = useArrivingView(viewKey);
  return arriving === null ? null : <div key={arriving} class="nav-cover" aria-hidden="true" />;
}

/** The class a pane's header title appends so it fades in with the pane's
 *  arriving view: `.nav-arrive`, the cover's mirror, on the same curve. Key the
 *  title element on the same `viewKey`, so each arrival is a fresh element
 *  whose animation starts in the same frame as the cover's. */
export function useArrivalFade(viewKey: string | null): string {
  return useArrivingView(viewKey) === null ? '' : ' nav-arrive';
}
