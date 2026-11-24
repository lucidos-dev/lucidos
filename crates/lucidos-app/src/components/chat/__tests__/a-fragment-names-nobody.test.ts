import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { chatExchangePropsEqual } from '../ChatExchange';
import { drawsInitiatorPanel, exchangeKey, type Exchange, type StoredEvent } from '../../../store/thread-events';

const here: string = dirname(fileURLToPath(import.meta.url));
const chatExchangeSource: string = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

const aToolCall = {
  type: 'CodingAgentToolCalled',
  channel: 'claude_code',
  name: 'Bash',
  tool_use_id: 'tool-1',
  _eventId: 'evt-1',
} as unknown as StoredEvent;

const fragment: Exchange = { userEvent: aToolCall, userSeq: 10, steps: [], continuationFragment: true };

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

  it('is what the component gates its initiator panel on', () => {
    expect(chatExchangeSource).toContain('{drawsInitiatorPanel(exchange) && (');
  });

  it('keys both panels, so the response panel survives the omission', () => {
    expect(chatExchangeSource).toContain('key="initiator"');
    expect(chatExchangeSource).toContain('key="response"');
  });

  it('gets a new render key when it becomes a real turn', () => {
    expect(exchangeKey(fragment)).toBe('id:evt-1');
    expect(exchangeKey(settled)).toBe('id:evt-0');
    expect(exchangeKey(fragment)).not.toBe(exchangeKey(settled));
  });

  it('carries no unreachable fragment compare in the memo', () => {
    expect(chatExchangeSource).not.toContain('a.continuationFragment !== b.continuationFragment');
  });

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
