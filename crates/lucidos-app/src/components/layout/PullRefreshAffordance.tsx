import { PULL_REFRESH_THRESHOLD_PX } from '@lucidos/pull-to-refresh';
import { panelPullTravel } from '../../store/panelRefresh';
import { ReloadIcon } from '../shared/icons';

export const PULL_OVERSHOOT_TURN_RATE = 1 / 3;

export function pullAffordanceStyle(travel: number): Record<string, string> {
  const progress = Math.min(1, travel / PULL_REFRESH_THRESHOLD_PX);
  const overshoot = Math.max(0, travel - PULL_REFRESH_THRESHOLD_PX) / PULL_REFRESH_THRESHOLD_PX;
  return {
    '--pull-travel': `${progress * PULL_REFRESH_THRESHOLD_PX}px`,
    '--pull-progress': String(progress),
    '--pull-turn': String(progress + overshoot * PULL_OVERSHOOT_TURN_RATE),
  };
}

export function PullRefreshAffordance() {
  const travel = panelPullTravel.value;
  const cls = `pull-refresh-affordance${travel > 0 ? ' is-pulling' : ''}`;
  return (
    <div class={cls} style={pullAffordanceStyle(travel)} aria-hidden="true">
      <ReloadIcon />
    </div>
  );
}
