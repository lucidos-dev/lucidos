import { PULL_REFRESH_THRESHOLD_PX } from '@lucidos/pull-to-refresh';
import { panelPullTravel } from '../../store/panelRefresh';
import { ReloadIcon } from '../shared/icons';

/** How fast the arrow turns once it holds its place, as a share of its
 *  turning speed while it drops. */
export const PULL_OVERSHOOT_TURN_RATE = 1 / 3;

/** The arrow's CSS variables for a pull that has travelled `travel` px. It
 *  drops and blends until a release would refresh, then holds its place. It
 *  keeps turning, more slowly, for as long as the finger keeps pulling. */
export function pullAffordanceStyle(travel: number): Record<string, string> {
  const progress = Math.min(1, travel / PULL_REFRESH_THRESHOLD_PX);
  const overshoot = Math.max(0, travel - PULL_REFRESH_THRESHOLD_PX) / PULL_REFRESH_THRESHOLD_PX;
  return {
    '--pull-travel': `${progress * PULL_REFRESH_THRESHOLD_PX}px`,
    '--pull-progress': String(progress),
    '--pull-turn': String(progress + overshoot * PULL_OVERSHOOT_TURN_RATE),
  };
}

/** The arrow that drops from the top of the content pane with a pull. It blends
 *  into the accent colour with the pull, fully there once a release would
 *  refresh. On release it springs back, and the header's refresh spinner takes
 *  over. Drawn by the host for pulls on the pane and inside an app frame alike. */
export function PullRefreshAffordance() {
  const travel = panelPullTravel.value;
  const cls = `pull-refresh-affordance${travel > 0 ? ' is-pulling' : ''}`;
  return (
    <div class={cls} style={pullAffordanceStyle(travel)} aria-hidden="true">
      <ReloadIcon />
    </div>
  );
}
