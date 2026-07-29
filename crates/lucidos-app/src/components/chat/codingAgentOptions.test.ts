import { describe, expect, it } from 'vitest';
import {
  availableReasoningOptions,
  reconcileReasoningEffort,
} from './codingAgentOptions';

const OPTIONS = [
  { value: 'low', label: 'Low', description: 'Fast' },
  { value: 'medium', label: 'Medium', description: 'Balanced' },
  { value: 'high', label: 'High', description: 'Deep' },
  { value: 'xhigh', label: 'Extra High', description: 'Deeper' },
  {
    value: 'max',
    label: 'Max',
    description: 'Maximum',
    supported_models: ['gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna'],
  },
];

describe('Codex model-scoped reasoning options', () => {
  it.each(['gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna'])(
    'offers Max for %s',
    model => {
      expect(availableReasoningOptions(OPTIONS, model).map(option => option.value))
        .toEqual(['low', 'medium', 'high', 'xhigh', 'max']);
    },
  );

  it.each(['gpt-5.5', 'gpt-5.4', 'gpt-5.4-mini', 'default', null])(
    'caps %s at Extra High',
    model => {
      expect(availableReasoningOptions(OPTIONS, model).map(option => option.value))
        .toEqual(['low', 'medium', 'high', 'xhigh']);
    },
  );

  it('reconciles Max to the strongest supported effort after a model switch', () => {
    expect(reconcileReasoningEffort('max', 'gpt-5.5', OPTIONS)).toBe('xhigh');
    expect(reconcileReasoningEffort('max', 'default', OPTIONS)).toBe('xhigh');
    expect(reconcileReasoningEffort('max', null, OPTIONS)).toBe('xhigh');
  });

  it('preserves supported and empty selections', () => {
    expect(reconcileReasoningEffort('max', 'gpt-5.6-sol', OPTIONS)).toBe('max');
    expect(reconcileReasoningEffort('high', 'gpt-5.5', OPTIONS)).toBe('high');
    expect(reconcileReasoningEffort(null, 'gpt-5.5', OPTIONS)).toBeNull();
  });
});
