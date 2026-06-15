import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, exchangeStatus, type StoredEvent } from '../thread-events';
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
    const events = exchangeResponseEvents(exchange, 0, true, false);
    expect(events.some(e => e.type === 'empty')).toBe(true);
    // The turn completed — not an error.
    expect(exchangeStatus(exchange, '', true)).toBe('done');
  });

  it('pushes an empty note even with no tool calls (model said nothing)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ResponseGenerated', text: '' }),
    ]);
    const events = exchangeResponseEvents(exchange, 0, true, false);
    expect(events.some(e => e.type === 'empty')).toBe(true);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
  });

  it('no note when the response has text', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'Playing it now.' }),
      step(2, { type: 'ResponseGenerated', text: 'Playing it now.' }),
    ]);
    const events = exchangeResponseEvents(exchange, 0, true, false);
    expect(events.some(e => e.type === 'empty')).toBe(false);
  });

  it('no note for the [ENGINE-LIMIT] cap (text-less stream but non-empty RG text)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ToolCalled', name: 'x', args: {} }),
      step(2, { type: 'ResponseGenerated', text: '[ENGINE-LIMIT] hit the per-turn cap' }),
    ]);
    const events = exchangeResponseEvents(exchange, 0, true, false);
    expect(events.some(e => e.type === 'empty')).toBe(false);
  });

  it('no note for a ResponseFailed exchange (stays a red error)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ToolCalled', name: 'x', args: {} }),
      step(2, { type: 'ResponseFailed', error: 'Model returned no response (stop_reason: max_tokens …).' }),
    ]);
    const events = exchangeResponseEvents(exchange, 0, true, false);
    expect(events.some(e => e.type === 'empty')).toBe(false);
    expect(exchangeStatus(exchange, '', true)).toBe('error');
  });
});
