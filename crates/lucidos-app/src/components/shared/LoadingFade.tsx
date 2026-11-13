import type { ComponentChildren, VNode } from 'preact';
import { useLingeringFlag, useMinimumVisibleFlag } from '../../hooks/useDelayedLoading';
import { scaledDurationMs } from '../../store/store';

/** The skeleton's CSS opacity fade at 1x speed: `var(--duration-normal)`. */
const SKELETON_FADE_MS = 200;
/** Fixed margin past the fade, so the element stays mounted until it ends. */
const SKELETON_FADE_SLACK_MS = 50;

/** Owns a skeleton's EXIT. The skeleton stays opaque for at least
 *  `LOADER_MIN_VISIBLE_MS` from when it appeared, so a load ending just past the
 *  delay gate does not flash it. It then crossfades out as content comes in, so
 *  a slow load does not hard-snap to content. `children` renders as soon as it is
 *  ready but stays hidden until the floor lifts, so loaded content never draws
 *  beside an opaque skeleton. Always pair with a delay gate
 *  (`useDelayedFlag`/`useDelayedLoading`) on `showSkeleton` so fast loads never show the skeleton at all — including
 *  full-screen surfaces (the workspace picker gates it too: its boot splash +
 *  header/footer chrome cover the brief pre-skeleton window, so an ungated
 *  skeleton just blinked under the clearing splash on a fast load). See
 *  `.claude/rules/frontend.md`.
 *
 *  Layout: the content wrapper and the skeleton share one grid cell (stacked).
 *  While loading, the skeleton sizes the cell (children is typically null); once
 *  loaded the content sizes it and the skeleton goes absolute + fades, so it
 *  doesn't fight the content's height. Reduced-motion: the skeleton disappears
 *  without animating (see styles/components.css).
 *
 *  `children` is wrapped in a single `.loading-fade-content` element so callers
 *  may pass a fragment of multiple siblings. Without the wrapper those siblings
 *  would each become a direct grid child and `.loading-fade > * { grid-area: 1/1
 *  }` would stack them ALL into the one cell, overlapping (the Changes-panel
 *  double-text bug). The wrapper keeps exactly two grid children — content and
 *  skeleton — regardless of what the caller passes. */
export function LoadingFade({
  showSkeleton,
  skeleton,
  class: cls,
  children,
}: {
  showSkeleton: boolean;
  skeleton: VNode;
  /** Extra class on the wrapper. The wrapper takes the place of whatever it
   *  wraps in the PARENT's layout, so any rule that used to reach the content
   *  as a flex/grid child now has to reach this instead (a control with
   *  `flex-shrink: 0` stops holding its width otherwise, since the wrapper
   *  shrinks in its place). */
  class?: string;
  children: ComponentChildren;
}) {
  const held = useMinimumVisibleFlag(showSkeleton);
  const skeletonMounted = useLingeringFlag(held, scaledDurationMs(SKELETON_FADE_MS) + SKELETON_FADE_SLACK_MS);
  const waitingOutFloor = held && !showSkeleton;
  const classes = ['loading-fade', cls, waitingOutFloor && 'loading-fade-held'].filter(Boolean).join(' ');
  return (
    <div class={classes}>
      <div class="loading-fade-content">{children}</div>
      {skeletonMounted && (
        <div class={`loading-fade-skeleton${held ? '' : ' loading-fade-out'}`} aria-hidden="true">
          {skeleton}
        </div>
      )}
    </div>
  );
}
