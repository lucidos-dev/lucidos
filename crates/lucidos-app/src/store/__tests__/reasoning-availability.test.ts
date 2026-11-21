import { describe, it, expect } from 'vitest';
import { REASONING_LEVELS, availableReasoningLevels } from '../models';

const all = REASONING_LEVELS.map(l => l.value);

// The registry's answer (a model's `reasoning_efforts` from /models, derived by
// `llm::reasoning::supported_efforts`) is the only source of a model's tiers.
// The client used to guess from the id's shape as well, and the two disagreed:
// a local model was offered `max`, which its server then rejected.
describe('availableReasoningLevels', () => {
  it('offers exactly the registry set', () => {
    const values = availableReasoningLevels(['none', 'low', 'medium', 'high']).map(l => l.value);
    expect(values).toEqual(['none', 'low', 'medium', 'high']);
  });

  it('keeps the levels in ladder order however the set is ordered', () => {
    const values = availableReasoningLevels(['high', 'none', 'medium']).map(l => l.value);
    expect(values).toEqual(['none', 'medium', 'high']);
  });

  it('offers the whole ladder with no registry answer, for the engine to snap', () => {
    expect(availableReasoningLevels().map(l => l.value)).toEqual(all);
  });

  it('offers the whole ladder rather than nothing for an unusable answer', () => {
    // An empty or unrecognisable set would render an empty dropdown, leaving
    // the user no way to pick at all.
    expect(availableReasoningLevels([]).map(l => l.value)).toEqual(all);
    expect(availableReasoningLevels(['nonsense']).map(l => l.value)).toEqual(all);
  });
});
