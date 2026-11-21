import { describe, it, expect } from 'vitest';
import { LOADER_MIN_VISIBLE_MS } from '../../../hooks/useDelayedLoading';
import { REFRESH_DONE_HOLD_MS, refreshEndSteps } from '../RefreshIndicator';

describe('refreshEndSteps', () => {
  it('keeps a quick refresh spinning for the minimum, then checks', () => {
    expect(refreshEndSteps(true, 100)).toEqual([
      { afterMs: LOADER_MIN_VISIBLE_MS - 100, state: 'done' },
      { afterMs: LOADER_MIN_VISIBLE_MS - 100 + REFRESH_DONE_HOLD_MS, state: 'idle' },
    ]);
  });

  it('checks at once after a refresh that already spun long enough', () => {
    expect(refreshEndSteps(true, LOADER_MIN_VISIBLE_MS + 500)).toEqual([
      { afterMs: 0, state: 'done' },
      { afterMs: REFRESH_DONE_HOLD_MS, state: 'idle' },
    ]);
  });

  it('never ends a refresh that did not land on a check', () => {
    expect(refreshEndSteps(false, 100)).toEqual([{ afterMs: LOADER_MIN_VISIBLE_MS - 100, state: 'idle' }]);
  });

  it('still spins, then checks, for a refresh that landed before a render saw it run', () => {
    expect(refreshEndSteps(true, null)).toEqual([
      { afterMs: 0, state: 'refreshing' },
      { afterMs: LOADER_MIN_VISIBLE_MS, state: 'done' },
      { afterMs: LOADER_MIN_VISIBLE_MS + REFRESH_DONE_HOLD_MS, state: 'idle' },
    ]);
  });

  it('shows nothing when no refresh ran since the last render', () => {
    expect(refreshEndSteps(false, null)).toEqual([]);
  });
});
