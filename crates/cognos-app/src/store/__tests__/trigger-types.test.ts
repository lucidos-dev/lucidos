import { describe, it, expect } from 'vitest';
import { deriveTriggerType, type TriggerInfo } from '../types';

function makeTrigger(overrides: Partial<TriggerInfo> = {}): TriggerInfo {
  return {
    id: '1',
    name: 'Test',
    cron_expressions: [],
    timezone: 'UTC',
    enabled: true,
    run: { type: 'intent', text: 'test', knowhow: [] },
    ...overrides,
  };
}

describe('deriveTriggerType', () => {
  it('returns schedule for cron-only trigger', () => {
    const trigger = makeTrigger({ cron_expressions: ['0 0 8 * * *'] });
    expect(deriveTriggerType(trigger)).toBe('schedule');
  });

  it('returns event for event-only trigger', () => {
    const trigger = makeTrigger({ on: 'OuraSleepImported' });
    expect(deriveTriggerType(trigger)).toBe('event');
  });

  it('returns hybrid for trigger with both cron and event', () => {
    const trigger = makeTrigger({
      cron_expressions: ['0 0 8 * * *'],
      on: 'OuraSleepImported',
    });
    expect(deriveTriggerType(trigger)).toBe('hybrid');
  });

  it('returns schedule when cron_expressions is non-empty and no on', () => {
    const trigger = makeTrigger({
      cron_expressions: ['0 0 8 * * *', '0 0 20 * * *'],
    });
    expect(deriveTriggerType(trigger)).toBe('schedule');
  });

  it('returns event when on is set and cron_expressions is empty', () => {
    const trigger = makeTrigger({
      cron_expressions: [],
      on: 'HealthDataImported',
      condition: { sleep_score: { $lt: 70 } },
    });
    expect(deriveTriggerType(trigger)).toBe('event');
  });
});
