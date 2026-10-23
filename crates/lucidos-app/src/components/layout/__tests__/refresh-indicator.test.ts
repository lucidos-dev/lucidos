import { describe, it, expect } from 'vitest';
import { REFRESH_DONE_HOLD_MS, REFRESH_MIN_SPIN_MS, refreshEndSteps } from '../RefreshIndicator';

describe('refreshEndSteps', () => {
  it('keeps a quick refresh spinning for the minimum, then checks', () => {
    expect(refreshEndSteps(true, 100)).toEqual([
      { afterMs: REFRESH_MIN_SPIN_MS - 100, state: 'done' },
      { afterMs: REFRESH_MIN_SPIN_MS - 100 + REFRESH_DONE_HOLD_MS, state: 'idle' },
    ]);
  });

  it('checks at once after a refresh that already spun long enough', () => {
    expect(refreshEndSteps(true, REFRESH_MIN_SPIN_MS + 500)).toEqual([
      { afterMs: 0, state: 'done' },
      { afterMs: REFRESH_DONE_HOLD_MS, state: 'idle' },
    ]);
  });

  it('never ends a refresh that did not land on a check', () => {
    expect(refreshEndSteps(false, 100)).toEqual([{ afterMs: REFRESH_MIN_SPIN_MS - 100, state: 'idle' }]);
  });

  it('still spins, then checks, for a refresh that landed before a render saw it run', () => {
    expect(refreshEndSteps(true, null)).toEqual([
      { afterMs: 0, state: 'refreshing' },
      { afterMs: REFRESH_MIN_SPIN_MS, state: 'done' },
      { afterMs: REFRESH_MIN_SPIN_MS + REFRESH_DONE_HOLD_MS, state: 'idle' },
    ]);
  });

  it('shows nothing when no refresh ran since the last render', () => {
    expect(refreshEndSteps(false, null)).toEqual([]);
  });
});
