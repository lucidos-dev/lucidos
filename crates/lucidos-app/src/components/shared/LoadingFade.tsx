import type { ComponentChildren, VNode } from 'preact';
import { useLingeringFlag, useMinimumVisibleFlag } from '../../hooks/useDelayedLoading';
import { scaledDurationMs } from '../../store/store';

const SKELETON_FADE_MS = 200;
const SKELETON_FADE_SLACK_MS = 50;

export function LoadingFade({
  showSkeleton,
  skeleton,
  class: cls,
  children,
}: {
  showSkeleton: boolean;
  skeleton: VNode;
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
