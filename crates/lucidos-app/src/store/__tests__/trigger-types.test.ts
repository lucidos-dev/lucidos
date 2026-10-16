import { describe, it, expect } from 'vitest';
import { deriveTriggerType } from '../types';
import { makeTrigger } from './fixtures';

describe('deriveTriggerType', () => {
  it('returns schedule for cron-only trigger', () => {
    const trigger = makeTrigger({ cron_expressions: ['0 0 8 * * *'] });
    expect(deriveTriggerType(trigger)).toBe('schedule');
  });

  it('returns event for event-only trigger', () => {
    const trigger = makeTrigger({ on: [{ event_type: 'OuraSleepImported' }] });
    expect(deriveTriggerType(trigger)).toBe('event');
  });

  it('returns hybrid for trigger with both cron and event', () => {
    const trigger = makeTrigger({
      cron_expressions: ['0 0 8 * * *'],
      on: [{ event_type: 'OuraSleepImported' }],
    });
    expect(deriveTriggerType(trigger)).toBe('hybrid');
  });

  it('returns schedule when cron_expressions is non-empty and no on entries', () => {
    const trigger = makeTrigger({
      cron_expressions: ['0 0 8 * * *', '0 0 20 * * *'],
    });
    expect(deriveTriggerType(trigger)).toBe('schedule');
  });

  it('returns event when on has at least one entry with per-event condition', () => {
    const trigger = makeTrigger({
      cron_expressions: [],
      on: [{ event_type: 'HealthDataImported', condition: { sleep_score: { $lt: 70 } } }],
    });
    expect(deriveTriggerType(trigger)).toBe('event');
  });

  it('returns event when on has multiple entries (multi-event trigger)', () => {
    const trigger = makeTrigger({
      cron_expressions: [],
      on: [
        { event_type: 'OuraSleepImported', condition: { sleep_score: { $lt: 70 } } },
        { event_type: 'EmailReceived' },
      ],
    });
    expect(deriveTriggerType(trigger)).toBe('event');
  });

  it('returns schedule when on is an empty array (not undefined)', () => {
    const trigger = makeTrigger({ cron_expressions: ['0 0 8 * * *'], on: [] });
    expect(deriveTriggerType(trigger)).toBe('schedule');
  });
});
