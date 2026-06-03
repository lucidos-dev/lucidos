import { describe, it } from 'vitest';
import type { Trigger, UpdateTrigger } from './triggers';

// Pin the SDK types to the engine's wire format
// (crates/lucidos-engine/src/api/triggers.rs `TriggerInfo`). The engine
// serializes the active-state flag as `paused: bool` — the `satisfies`
// clauses below fail at tsc time if the SDK declares it under a
// different name. The type-check IS the test; returning the literal
// dodges `noUnusedLocals` without forcing a vestigial runtime expect.
describe('Trigger type matches engine wire format', () => {
  it('has `paused` (not `enabled`) on the response shape', () =>
    ({
      id: 't1',
      name: 'Daily summary',
      cron_expressions: ['0 0 8 * * *'],
      timezone: 'UTC',
      paused: false,
      run: { type: 'intent' as const, intent: 'do the thing' },
    }) satisfies Trigger);
});

describe('UpdateTrigger type matches engine request format', () => {
  it('accepts `paused` to mirror the wire field the engine reads first', () =>
    ({ paused: true }) satisfies UpdateTrigger);
});
