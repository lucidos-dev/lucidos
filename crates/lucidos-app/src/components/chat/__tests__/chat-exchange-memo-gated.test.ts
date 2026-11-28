
import { describe, it, expect } from 'vitest';
import { chatExchangePropsEqual } from '../ChatExchange';
import type { Exchange } from '../../../store/thread-events';

const STEP = { seq: 7, event: { type: 'CodingAgentToolCalled', created: 'T' } as never };

function exchange(over: Partial<Exchange> = {}): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'hi', created: 'T', _eventId: 'm1' } as never,
    userSeq: 1,
    steps: [STEP],
    ...over,
  };
}

const props = (ex: Exchange) => ({ exchange: ex, revision: 0 } as never);

const equal = (a: Exchange, b: Exchange) => chatExchangePropsEqual(props(a), props(b));

describe('the memo sees a permission decision on a call it owns', () => {
  it('re-renders when a card holds the call, with no revision to help', () => {
    const before = exchange();
    const after = exchange({ blockedStepSeqs: new Set([7]) });
    expect(equal(before, after)).toBe(false);
  });

  it('re-renders when the hold is released', () => {
    const held = exchange({ blockedStepSeqs: new Set([7]) });
    const allowed = exchange({ blockedStepSeqs: new Set() });
    expect(equal(held, allowed)).toBe(false);
  });

  it('re-renders when the decision was no', () => {
    const held = exchange({ blockedStepSeqs: new Set([7]) });
    const denied = exchange({ blockedStepSeqs: new Set(), deniedStepSeqs: new Set([7]) });
    expect(equal(held, denied)).toBe(false);
  });

  it('re-renders when a second call is held beside the first', () => {
    const one = exchange({ blockedStepSeqs: new Set([7]) });
    const two = exchange({ blockedStepSeqs: new Set([7, 9]) });
    expect(equal(one, two)).toBe(false);
  });
});

describe('and does not re-render for a difference that is not one', () => {
  it('treats an absent set and an empty set as the same state', () => {
    expect(equal(exchange(), exchange({ blockedStepSeqs: new Set() }))).toBe(true);
    expect(equal(exchange({ deniedStepSeqs: new Set() }), exchange())).toBe(true);
  });

  it('treats equal sets as equal across a rebuild that reallocated them', () => {
    const a = exchange({ blockedStepSeqs: new Set([7]), deniedStepSeqs: new Set([3]) });
    const b = exchange({ blockedStepSeqs: new Set([7]), deniedStepSeqs: new Set([3]) });
    expect(equal(a, b)).toBe(true);
  });
});
