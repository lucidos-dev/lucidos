import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, exchangeResponseText, exchangeStatus, type StoredEvent } from '../thread-events';
import { makeExchange, step } from './fixtures';

function msg(text = 'play tåpelig'): StoredEvent {
  return { type: 'MessageReceived', text };
}

// A benign empty completion (the model ended its turn cleanly with no text) is
// emitted as an empty ResponseGenerated. The renderer must state that plainly
// with a neutral `empty` note — NOT a red error — while a real ResponseFailed
// and the [ENGINE-LIMIT] cap must keep their existing rendering.
describe('benign empty completion → neutral note', () => {
  it('pushes an empty note when ResponseGenerated has no text after tool calls', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ToolCalled', name: 'proxy_request', args: {} }),
      step(2, { type: 'ToolResult', name: 'proxy_request', result: '{"status":"success"}' } as StoredEvent),
      step(3, { type: 'ResponseGenerated', text: '' }),
    ]);
    const events = exchangeResponseEvents(exchange, true, false);
    expect(events.some(e => e.type === 'empty')).toBe(true);
    // The turn completed — not an error.
    expect(exchangeStatus(exchange, '', true)).toBe('done');
  });

  it('pushes an empty note even with no tool calls (model said nothing)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ResponseGenerated', text: '' }),
    ]);
    const events = exchangeResponseEvents(exchange, true, false);
    expect(events.some(e => e.type === 'empty')).toBe(true);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
  });

  it('no note when the response has text', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'Playing it now.' }),
      step(2, { type: 'ResponseGenerated', text: 'Playing it now.' }),
    ]);
    const events = exchangeResponseEvents(exchange, true, false);
    expect(events.some(e => e.type === 'empty')).toBe(false);
  });

  it('no note when a quiet pick ended the turn, but a loud pick still gets one', () => {
    const answeredWith = (quiet: boolean) => makeExchange(msg(), [
      step(1, { type: 'ToolCalled', name: 'ask_user_question', args: {} }),
      step(2, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'q#q0',
        answer: { kind: 'Selected', option_id: 'opt-1' },
        quiet,
      } as StoredEvent),
      step(3, { type: 'ResponseGenerated', text: '' }),
    ]);
    const hasNote = (quiet: boolean) =>
      exchangeResponseEvents(answeredWith(quiet), true, false).some(e => e.type === 'empty');
    expect(hasNote(true)).toBe(false);
    expect(hasNote(false)).toBe(true);
  });

  // The engine ends a quiet pick on its quiet reply as ResponseGenerated.text,
  // with no streamed chunk before it, on every agent.
  describe('a quiet reply renders as the turn\'s text', () => {
    const answered = (quiet: boolean) => step(2, {
      type: 'UserQuestionAnswered',
      tool_use_id: 'q#q0',
      answer: { kind: 'Selected', option_id: 'opt-1' },
      quiet,
    } as StoredEvent);
    const quietAnswer = answered(true);
    const texts = (events: ReturnType<typeof exchangeResponseEvents>) =>
      events.flatMap(e => (e.type === 'text' ? [e.md] : []));

    it('draws the reply text when nothing streamed after the pick', () => {
      const exchange = makeExchange(msg(), [
        step(1, { type: 'CodingAgentTextStreamed', text: 'Should I spawn a follow-up?' }),
        quietAnswer,
        step(3, { type: 'ResponseGenerated', text: 'Left as known limitations.' }),
      ]);
      const events = exchangeResponseEvents(exchange, true, false);
      expect(texts(events).join('')).toContain('Left as known limitations.');
      expect(events.some(e => e.type === 'empty')).toBe(false);
    });

    it('does not draw the text twice when the agent streamed it after the pick', () => {
      const exchange = makeExchange(msg(), [
        quietAnswer,
        step(3, { type: 'TextStreamed', text: 'Noted.' }),
        step(4, { type: 'ResponseGenerated', text: 'Noted.' }),
      ]);
      expect(texts(exchangeResponseEvents(exchange, true, false))).toEqual(['Noted.']);
    });

    it('still draws the reply when the agent streamed different text after the pick', () => {
      const exchange = makeExchange(msg(), [
        quietAnswer,
        step(3, { type: 'CodingAgentTextStreamed', text: 'Understood.' }),
        step(4, { type: 'ResponseGenerated', text: 'Left as known limitations.' }),
      ]);
      expect(texts(exchangeResponseEvents(exchange, true, false)).join('')).toContain('Left as known limitations.');
    });

    it('the copied response text carries the reply', () => {
      const exchange = makeExchange(msg(), [
        quietAnswer,
        step(3, { type: 'ResponseGenerated', text: 'Left as known limitations.' }),
      ]);
      expect(exchangeResponseText(exchange)).toBe('Left as known limitations.');
    });

    it('a loud pick never draws ResponseGenerated text', () => {
      const exchange = makeExchange(msg(), [
        answered(false),
        step(3, { type: 'ResponseGenerated', text: 'final summary' }),
      ]);
      expect(texts(exchangeResponseEvents(exchange, true, false))).toEqual([]);
    });
  });

  it('no note for the [ENGINE-LIMIT] cap (text-less stream but non-empty RG text)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ToolCalled', name: 'x', args: {} }),
      step(2, { type: 'ResponseGenerated', text: '[ENGINE-LIMIT] hit the per-turn cap' }),
    ]);
    const events = exchangeResponseEvents(exchange, true, false);
    expect(events.some(e => e.type === 'empty')).toBe(false);
  });

  it('no note for a ResponseFailed exchange (stays a red error)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ToolCalled', name: 'x', args: {} }),
      step(2, { type: 'ResponseFailed', error: 'Model returned no response (stop_reason: max_tokens …).' }),
    ]);
    const events = exchangeResponseEvents(exchange, true, false);
    expect(events.some(e => e.type === 'empty')).toBe(false);
    expect(exchangeStatus(exchange, '', true)).toBe('error');
  });
});
