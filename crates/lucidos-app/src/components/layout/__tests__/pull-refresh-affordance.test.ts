import { describe, it, expect } from 'vitest';
import { PULL_REFRESH_THRESHOLD_PX } from '@lucidos/pull-to-refresh';
import { PULL_OVERSHOOT_TURN_RATE, pullAffordanceStyle } from '../PullRefreshAffordance';

describe('pullAffordanceStyle', () => {
  it('drops, blends and turns together below the threshold', () => {
    expect(pullAffordanceStyle(PULL_REFRESH_THRESHOLD_PX / 2)).toEqual({
      '--pull-travel': `${PULL_REFRESH_THRESHOLD_PX / 2}px`,
      '--pull-progress': '0.5',
      '--pull-turn': '0.5',
    });
  });

  it('stops dropping at the threshold but keeps turning, more slowly, with the finger', () => {
    expect(pullAffordanceStyle(PULL_REFRESH_THRESHOLD_PX * 3)).toEqual({
      '--pull-travel': `${PULL_REFRESH_THRESHOLD_PX}px`,
      '--pull-progress': '1',
      '--pull-turn': String(1 + 2 * PULL_OVERSHOOT_TURN_RATE),
    });
  });

  it('turns slower once it stops dropping than while it drops', () => {
    const turn = (travel: number) => Number(pullAffordanceStyle(travel)['--pull-turn']);
    const t = PULL_REFRESH_THRESHOLD_PX;
    expect(turn(2 * t) - turn(t)).toBeLessThan(turn(t) - turn(0));
  });
});
