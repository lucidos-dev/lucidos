import { describe, it, expect } from 'vitest';
import { makeExchange, step } from '../../__tests__/fixtures';
import { exchangeResponseEvents, exchangeStatus, resumeEngineNote } from '../exchange-render';
import type { StoredEvent } from '../../thread-events';

const msg = (text = 'go'): StoredEvent => ({ type: 'MessageReceived', text } as StoredEvent);

describe('exchange render edges', () => {
  it('draws no empty note when a ResponseGenerated carries text the stream never sent', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ToolCalled', name: 'proxy_request', args: {} } as StoredEvent),
      step(2, { type: 'ResponseGenerated', text: '[ENGINE-LIMIT] Per-turn limit of 500 tool calls reached.' } as StoredEvent),
    ]);
    const events = exchangeResponseEvents(exchange, true, false);
    expect(events.some(e => e.type === 'empty')).toBe(false);
  });

  it('ends a coding-agent session on a panic as aborted, not done', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' } as StoredEvent),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as StoredEvent),
      step(3, { type: 'SessionEnded', reason: 'panic' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('aborted');
  });

  it('reads a coding agent that works on after going idle as working again', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' } as StoredEvent),
      step(2, { type: 'CodingAgentTextStreamed', text: 'Looking.' } as StoredEvent),
      step(3, { type: 'CodingAgentIdled', has_changes: false } as StoredEvent),
      step(4, { type: 'CodingAgentTextStreamed', text: 'Actually, one more.' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('coding-agent-working');
  });

  it('reads an abort after a legacy same-exchange follow-up as aborted, not done', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' } as StoredEvent),
      step(2, { type: 'CodingAgentIdled', has_changes: false } as StoredEvent),
      step(3, { type: 'CodingAgentUserMessageSent', text: 'and one more' } as StoredEvent),
      step(4, { type: 'CodingAgentTextStreamed', text: 'On it.' } as StoredEvent),
      step(5, { type: 'ResponseAborted' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('aborted');
  });

  it('keeps a turn read done when a late tool result lands after the agent idled', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' } as StoredEvent),
      step(2, { type: 'CodingAgentTextStreamed', text: 'Done.' } as StoredEvent),
      step(3, { type: 'CodingAgentIdled', has_changes: false } as StoredEvent),
      step(4, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('done');
  });

  it('counts only engine note lines that are bullets carrying an arrow result', () => {
    const text = [
      'Resume after restart. Work already done:',
      '- read_file(a.ts) → 40 lines',
      'a stray line → with an arrow',
      '- note without a result',
      '- list_files(src) → 3 entries',
    ].join('\n');
    const exchange = makeExchange(msg(), [
      step(1, { type: 'PromptInjected', mode: 'engine', text } as StoredEvent),
    ]);
    expect(resumeEngineNote(exchange)?.toolCount).toBe(2);
  });
});
