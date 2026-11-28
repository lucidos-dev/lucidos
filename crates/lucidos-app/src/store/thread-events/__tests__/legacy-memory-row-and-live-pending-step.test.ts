import { describe, it, expect } from 'vitest';
import { exchangeSteps, groupIntoExchanges, type ThreadEvent } from '../../thread-events';

describe('exchange steps', () => {
  it('a legacy MemorySearched row renders the recalled-memory label', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'hi' } as ThreadEvent],
      [2, { type: 'MemorySearched', results: 2 } as unknown as ThreadEvent],
    ]);
    const [ex] = groupIntoExchanges(events);
    const steps = exchangeSteps(ex, false, true);
    expect(steps.map(s => s.description)).toContain('Recalled 2 memories');
  });

  it('a non-last exchange with no terminator and a live thread keeps its pending step', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'find the file' } as ThreadEvent],
      [2, { type: 'ToolCalled', name: 'bash', args: { command: 'ls' } } as ThreadEvent],
      [3, { type: 'MessageReceived', text: 'next' } as ThreadEvent],
    ]);
    const [ex0] = groupIntoExchanges(events);
    const steps = exchangeSteps(ex0, false, false);
    expect(steps.filter(s => s.outcome === 'pending')).toHaveLength(1);
  });
});
