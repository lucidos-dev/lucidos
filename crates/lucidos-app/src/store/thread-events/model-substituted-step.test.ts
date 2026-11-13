import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, exchangeSteps } from './exchange-render';
import type { Exchange } from './exchange';

/** A coding agent running another model than the pick is visible, never
 *  hidden: the model button moves, and the transcript says why at that step. */

const TS = '2026-11-13T09:00:00Z';

function ccExchange(): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'carry on', created: TS, channel: 'claude_code', _eventId: 'msg-1' } as never,
    userSeq: 0,
    steps: [{
      seq: 1,
      event: {
        created: TS,
        type: 'CodingAgentModelSubstituted',
        requested: 'claude-pinned-model-x',
        served: 'claude-served-model-y',
        coding_agent: 'claude-code',
      } as never,
    }],
  };
}

describe('a coding-agent model substitution', () => {
  it('renders as a finished step naming both models', () => {
    const label = 'Ran claude-served-model-y, not claude-pinned-model-x: this account cannot use claude-pinned-model-x';
    const steps = exchangeResponseEvents(ccExchange(), true, true).filter((e) => e.type === 'step');
    expect(steps).toHaveLength(1);
    expect(steps[0]).toMatchObject({ description: label, outcome: 'success' });
    expect(exchangeSteps(ccExchange(), true, true).map((s) => s.description)).toEqual([label]);
  });
});
