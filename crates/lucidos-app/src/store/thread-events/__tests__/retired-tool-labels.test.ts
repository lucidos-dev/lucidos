import { describe, expect, it } from 'vitest';
import { describeEngineTool, fullCommandForEngineTool } from '../tool-descriptions';

describe('describeEngineTool: retired execute_intent', () => {
  it('labels a stored call by its intent', () => {
    expect(describeEngineTool('execute_intent', { intent_id: 'habit-tracker/log' }))
      .toBe('Run intent: habit-tracker/log');
    expect(fullCommandForEngineTool('execute_intent', { intent_id: 'habit-tracker/log' }))
      .toBe('habit-tracker/log');
  });

  it('labels a call with no intent id', () => {
    expect(describeEngineTool('execute_intent', {})).toBe('Run intent');
  });
});
