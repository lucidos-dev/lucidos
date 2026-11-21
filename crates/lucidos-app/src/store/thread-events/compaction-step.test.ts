import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, exchangeSteps } from './exchange-render';
import type { Exchange } from './exchange';

/** A coding agent compacting its own context is visible, never hidden: the
 *  context counter drops, and the transcript says why at that step. */

const TS = '2026-10-14T09:00:00Z';

function ccExchange(compacted: Record<string, unknown>): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'carry on', created: TS, channel: 'claude_code', _eventId: 'msg-1' } as never,
    userSeq: 0,
    steps: [{ seq: 1, event: { created: TS, type: 'CodingAgentCompacted', trigger: 'auto', ...compacted } as never }],
  };
}

describe('a coding-agent compaction', () => {
  it('renders as a finished step naming both sizes', () => {
    const exchange = ccExchange({ pre_tokens: 384_213, post_tokens: 58_702 });
    const steps = exchangeResponseEvents(exchange, true, true).filter((e) => e.type === 'step');
    expect(steps).toHaveLength(1);
    expect(steps[0]).toMatchObject({
      description: 'Compacted its context from 384k to 59k tokens',
      outcome: 'success',
    });
    expect(exchangeSteps(exchange, true, true).map((s) => s.description)).toEqual([
      'Compacted its context from 384k to 59k tokens',
    ]);
  });

  it('names only what Claude Code reported', () => {
    const steps = exchangeResponseEvents(ccExchange({ pre_tokens: 120_000 }), true, true)
      .filter((e) => e.type === 'step');
    expect(steps[0]).toMatchObject({ description: 'Compacted its context from 120k tokens' });
  });
});
