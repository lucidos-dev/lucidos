import type { TriggerInfo } from '../types';
import type { Exchange, StoredEvent, SequencedEvent } from '../thread-events';

export function makeTrigger(overrides: Partial<TriggerInfo> = {}): TriggerInfo {
  return {
    id: '1',
    name: 'Test',
    cron_expressions: [],
    timezone: 'UTC',
    paused: false,
    run: { type: 'intent', intent: 'test' },
    ...overrides,
  };
}

export function makeExchange(
  userEvent: StoredEvent,
  steps: Array<{ seq: number; event: StoredEvent }> = [],
): Exchange {
  return { userEvent, userSeq: 0, steps };
}

export function step(seq: number, event: StoredEvent): SequencedEvent {
  return { seq, event };
}
