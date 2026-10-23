/** A continuation fragment draws its rows and no initiator panel.
 *
 *  The fold opens one when a paged thread's newest page starts mid-turn. Its
 *  `userEvent` is its own first step, standing in for a render key, because
 *  nothing loaded says who started the turn. Drawing that as an initiator would
 *  put a tool call where the reader expects a person.
 *
 *  Read off the decision itself, plus one source scan checking the component
 *  still consults it. Same shape as `liveRowDrawsNoPanel`'s test beside this
 *  one, and for the same reason: rendering the whole component needs a store.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { chatExchangePropsEqual, drawsInitiatorPanel } from '../ChatExchange';
import { exchangeKey, type Exchange, type StoredEvent } from '../../../store/thread-events';

const here: string = dirname(fileURLToPath(import.meta.url));
const chatExchangeSource: string = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

const aToolCall = {
  type: 'CodingAgentToolCalled',
  channel: 'claude_code',
  name: 'Bash',
  tool_use_id: 'tool-1',
  _eventId: 'evt-1',
} as unknown as StoredEvent;

/** The fragment exactly as the fold opens it. */
const fragment: Exchange = { userEvent: aToolCall, userSeq: 10, steps: [], continuationFragment: true };

/** The same rows once the page behind them has landed. */
const settled: Exchange = {
  userEvent: { type: 'MessageReceived', text: 'do it', _eventId: 'evt-0' } as StoredEvent,
  userSeq: 9,
  steps: [],
};

describe('a continuation fragment', () => {
  it('draws no panel naming who started the turn', () => {
    expect(drawsInitiatorPanel(fragment)).toBe(false);
  });

  it('leaves every ordinary turn drawing one', () => {
    expect(drawsInitiatorPanel(settled)).toBe(true);
  });

  /** A source scan, because the decision is useless if the component stops
   *  asking it. The tripwire fails if the gate is renamed or dropped. */
  it('is what the component gates its initiator panel on', () => {
    expect(chatExchangeSource).toContain('{drawsInitiatorPanel(exchange) && (');
  });

  /** The two panels are keyed, so omitting the first cannot make Preact diff
   *  the second against it and rebuild it. See `.claude/rules/frontend.md`. */
  it('keys both panels, so the response panel survives the omission', () => {
    expect(chatExchangeSource).toContain('key="initiator"');
    expect(chatExchangeSource).toContain('key="response"');
  });

  /** The panel has to appear once the page carrying the boundary lands, and
   *  the MEMO is not what delivers it. A fragment keys off its own first step.
   *  The turn it merges into keys off the boundary, so Preact is handed a
   *  different key and remounts. `chatExchangePropsEqual` is never asked. */
  it('gets a new render key when it becomes a real turn', () => {
    expect(exchangeKey(fragment)).toBe('id:evt-1');
    expect(exchangeKey(settled)).toBe('id:evt-0');
    expect(exchangeKey(fragment)).not.toBe(exchangeKey(settled));
  });

  /** So the memo must not carry a `continuationFragment` compare: it could only ever
   *  answer with itself. */
  it('carries no unreachable fragment compare in the memo', () => {
    expect(chatExchangeSource).not.toContain('a.continuationFragment !== b.continuationFragment');
  });

  /** What the memo DOES owe a fragment is every ordinary term, since a live
   *  fragment gains steps like any other turn. */
  it('still re-renders a fragment as its steps arrive', () => {
    const props = { revision: 0, threadId: 't', streamingBuffer: '' };
    const before = { ...props, exchange: fragment };
    const after = { ...props, exchange: { ...fragment, steps: [{ seq: 11, event: aToolCall }] } };

    expect(chatExchangePropsEqual(
      before as unknown as Parameters<typeof chatExchangePropsEqual>[0],
      after as unknown as Parameters<typeof chatExchangePropsEqual>[1],
    )).toBe(false);
  });
});
