/** A turn's body rows carry the seq of the event behind them. That is how a
 *  side question sits between the rows written before and after it. */
import { beforeEach, describe, expect, it } from 'vitest';
import { getExchanges, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeResponseEvents } from '../thread-events';

beforeEach(resetSeqCounter);

function seqsOf(map: ReturnType<typeof makeThread>['map'], id: string): number[] {
  return [...map.get(id)!.events.keys()];
}

describe('response event seq', () => {
  it('stamps each row with its own event seq, and a merged chunk with its first', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'go' },
      { type: 'TextStreamed', text: 'Looking ' },
      { type: 'TextStreamed', text: 'now.' },
      { type: 'ToolCalled', name: 'list_files', args: { path: '.' } },
      { type: 'ToolResult', name: 'list_files', result: 'a.txt' },
      { type: 'ResponseGenerated' },
    ]);
    const [, firstText, , called] = seqsOf(map, id);
    const rows = exchangeResponseEvents(getExchanges(map, id)[0]);
    expect(rows.map((r) => [r.type, r.seq])).toEqual([
      ['text', firstText],
      ['step', called],
    ]);
  });

  it('gives a Thinking row renamed to a call the call\'s own seq', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'go' },
      { type: 'ThoughtStreamed', text: 'hmm' },
      { type: 'ToolCalled', name: 'list_files', args: { path: '.' } },
      { type: 'ToolResult', name: 'list_files', result: 'a.txt' },
      { type: 'ResponseGenerated' },
    ]);
    const [, , called] = seqsOf(map, id);
    const steps = exchangeResponseEvents(getExchanges(map, id)[0]).filter((r) => r.type === 'step');
    expect(steps).toHaveLength(1);
    expect(steps[0].seq).toBe(called);
  });

  it('leaves the live Thinking row unstamped, so it reads as the newest', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'go' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, tool_use_id: 'tu-1' },
      { type: 'CodingAgentToolResult', name: 'Read', tool_use_id: 'tu-1', result: 'ok' },
    ]);
    const rows = exchangeResponseEvents(getExchanges(map, id)[0], true, false);
    const last = rows[rows.length - 1];
    expect(last).toMatchObject({ type: 'step', description: 'Thinking', outcome: 'pending' });
    expect(last.seq).toBeUndefined();
    expect(rows[0].seq).toBe(seqsOf(map, id)[1]);
  });
});
