/** A child moved to top level, as its former parent's transcript shows it
 *  (ADR 0278). Like a stopped child's note, it wakes nothing. So it is a turn
 *  of its own, done at once, owning none of the work around it. */
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

  it('reads as done and leaves a running parent turn its own work', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'spawn a child for the notes' },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'notes.md' } },
      movedOut,
      { type: 'TodoListWritten', items: [{ content: 'check', status: 'in_progress' }] },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    const note = exchanges[1];
    expect(note.userEvent.type).toBe('ChildThreadDetached');
    expect(note.steps).toHaveLength(0);
    expect(exchangeStatus(note, '', true, false, false, false)).toBe('done');
    expect(exchanges[0].steps.map((s) => s.event.type)).toEqual(['ToolCalled', 'TodoListWritten']);
  });
});
