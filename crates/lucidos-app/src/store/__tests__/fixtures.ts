import type { TriggerInfo } from '../types';

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
