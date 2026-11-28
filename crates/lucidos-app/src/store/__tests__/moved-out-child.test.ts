import { beforeEach, describe, expect, it } from 'vitest';
import { getExchanges, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeStatus, isTurnlessBoundary, type ThreadEvent } from '../thread-events';

beforeEach(resetSeqCounter);

const movedOut = {
  type: 'ChildThreadDetached',
  child_thread_id: '00000000-0000-4000-8000-000000000001',
  child_thread_title: 'Write the notes',
} as ThreadEvent;

describe('a ChildThreadDetached note on the former parent', () => {
  it('is a turnless boundary', () => {
    expect(isTurnlessBoundary(movedOut)).toBe(true);
  });

  it('sits between the steps of a running parent turn', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'spawn a child for the notes' },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'notes.md' } },
      movedOut,
      { type: 'TodoListWritten', items: [{ content: 'check', status: 'in_progress' }] },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    const [turn] = exchanges;
    expect(turn.userEvent.type).toBe('MessageReceived');
    expect(turn.steps.map(({ event }) => event.type)).toEqual([
      'ToolCalled',
      'ChildThreadDetached',
      'TodoListWritten',
    ]);
    expect(exchangeStatus(turn, '', true)).toBe('streaming');
  });

  it('reads as done when it stands alone', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [movedOut]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    const [note] = exchanges;
    expect(note.userEvent.type).toBe('ChildThreadDetached');
    expect(note.steps).toHaveLength(0);
    expect(exchangeStatus(note, '', true)).toBe('done');
  });
});
