import { describe, it } from 'vitest';
import type { Trigger, UpdateTrigger } from './triggers';

describe('Trigger type matches engine wire format', () => {
  it('has `paused` (not `enabled`) on the response shape', () =>
    ({
      id: 't1',
      name: 'Daily summary',
      slug: 'daily-summary',
      cron_expressions: ['0 0 8 * * *'],
      timezone: 'UTC',
      paused: false,
      coding_agent_grant: false,
      run: { type: 'intent' as const, intent: 'do the thing' },
    }) satisfies Trigger);
});

describe('UpdateTrigger type matches engine request format', () => {
  it('accepts `paused` to mirror the wire field the engine reads first', () =>
    ({ paused: true }) satisfies UpdateTrigger);
});
