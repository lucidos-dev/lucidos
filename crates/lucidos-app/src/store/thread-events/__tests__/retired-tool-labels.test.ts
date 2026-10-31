import { describe, expect, it } from 'vitest';
import { describeEngineTool, fullCommandForEngineTool } from '../tool-descriptions';

// The retired `execute_intent` tool remains in stored threads with its
// calls. Their rows must keep a readable label rather than the bare tool name.
describe('describeEngineTool: retired execute_intent', () => {
  it('labels a stored call by its intent', () => {
    expect(describeEngineTool('execute_intent', { intent_id: 'habit-tracker/log' })) // Retired compatibility.
      .toBe('Run intent: habit-tracker/log');
    expect(fullCommandForEngineTool('execute_intent', { intent_id: 'habit-tracker/log' })) // Retired compatibility.
      .toBe('habit-tracker/log');
  });

  it('labels a call with no intent id', () => {
    expect(describeEngineTool('execute_intent', {})).toBe('Run intent'); // Retired compatibility.
  });
});
