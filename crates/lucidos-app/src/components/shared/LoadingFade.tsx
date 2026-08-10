import type { ComponentChildren, VNode } from 'preact';
import { useLingeringFlag } from '../../hooks/useDelayedLoading';

/** How long the skeleton lingers (fading out) after the load completes. Slightly
 *  longer than the CSS opacity transition (var(--duration-normal) = 200ms) so the
 *  element stays mounted until the fade finishes. */
const SKELETON_FADE_MS = 250;

/** Crossfades a skeleton OUT as content comes in — the smooth exit that a plain
 *  delayed loader lacks (a slow load otherwise hard-snaps from skeleton to
 *  content). No content is withheld: `children` (the loaded content) renders
 *  immediately when ready; the skeleton just lingers briefly and fades out over
 *  it. Always pair with a delay-only gate (`useDelayedFlag`/`useDelayedLoading`)
 *  on `showSkeleton` so fast loads never show the skeleton at all — including
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
  const skeletonMounted = useLingeringFlag(showSkeleton, SKELETON_FADE_MS);
  return (
    <div class={cls ? `loading-fade ${cls}` : 'loading-fade'}>
      <div class="loading-fade-content">{children}</div>
      {skeletonMounted && (
        <div class={`loading-fade-skeleton${showSkeleton ? '' : ' loading-fade-out'}`} aria-hidden="true">
          {skeleton}
        </div>
      )}
    </div>
  );
}
