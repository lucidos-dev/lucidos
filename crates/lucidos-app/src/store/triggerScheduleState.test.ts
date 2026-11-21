import { describe, it, expect } from 'vitest';
import { hasNoMoreRuns } from './types';
import { makeTrigger } from './__tests__/fixtures';

describe('hasNoMoreRuns', () => {
  it('is true for an active schedule-only trigger with nothing upcoming', () => {
    // A one-shot that already fired: its cron matched a single past moment.
    expect(hasNoMoreRuns(makeTrigger({ cron_expressions: ['0 0 9 4 8 *'] }))).toBe(true);
  });

  it('is false while a run is still upcoming', () => {
    expect(hasNoMoreRuns(makeTrigger({ next_run: '2030-01-01T09:00:00Z' }))).toBe(false);
  });

  it('is false for a paused trigger', () => {
    // Paused reads as "Paused", not "No more runs": resuming brings it back.
    expect(hasNoMoreRuns(makeTrigger({ paused: true }))).toBe(false);
  });

  it('is false for an event trigger, which has no schedule to exhaust', () => {
    expect(hasNoMoreRuns(makeTrigger({ on: [{ event_type: 'X' }] }))).toBe(false);
  });

  it('is false when the schedule can never fire, so the error chip wins', () => {
    // Both states have no next_run, but they mean opposite things: a spent
    // one-shot did its job, a never-firing cron never worked at all.
    const dead = makeTrigger({
      cron_expressions: ['0 0 9 31 2 *'],
      schedule_error:
        "'0 0 9 31 2 *' can never fire: day-of-month 31 never occurs in month 2 (February)",
    });
    expect(hasNoMoreRuns(dead)).toBe(false);
  });
});
