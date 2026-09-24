/** A *stopped child* in the transcript and the attention badge (ADR 0252).
 *
 *  On the parent, a `ChildThreadStopped` is a note that wakes nothing: its own
 *  turn, done at once, owning none of the work around it. On the child, the
 *  projection's `isStoppedChild` flag counts toward attention, because the
 *  parent stays asleep until the user acts. */
import { beforeEach, describe, expect, it } from 'vitest';
import { getExchanges, getLabel, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeStatus, isTurnlessBoundary, type ThreadEvent } from '../thread-events';
import { threadNeedsAttention } from '../store';

beforeEach(resetSeqCounter);

const CHILD_ID = '00000000-0000-4000-8000-000000000001';

const stoppedNote = {
  type: 'ChildThreadStopped',
  child_thread_id: CHILD_ID,
  child_thread_title: 'Fix the ticket',
} as ThreadEvent;

describe('a ChildThreadStopped note on the parent', () => {
  it('is a turnless boundary, like the Stop waiting row', () => {
    expect(isTurnlessBoundary(stoppedNote)).toBe(true);
    expect(isTurnlessBoundary({ type: 'ChildThreadCompleted' })).toBe(false);
  });

  /** It opens its own turn, and that turn is done whatever the thread is
   *  doing: no response follows a note that wakes nothing, so it must never
   *  spin "Requesting" or fall through to the stale detector. */
  it.each([
    ['idle', true],
    ['running', false],
  ] as const)('reads as done on a %s parent', (_label, threadIdle) => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'spawn a child for the ticket' },
      { type: 'ResponseGenerated', text: 'Spawned it.' },
      stoppedNote,
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    const note = exchanges[1];
    expect(note.userEvent.type).toBe('ChildThreadStopped');
    expect(exchangeStatus(note, '', true, false, false, threadIdle)).toBe('done');
    expect(getLabel(note)).toBe('Done');
  });

  /** A parent mid-turn when its child is stopped keeps writing where it was.
   *  Folded into the note, its work would draw nothing, since the note's panel
   *  renders no response body. */
  it('leaves a running parent turn owning its own work', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'keep an eye on the child' },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'notes.md' } },
      stoppedNote,
      { type: 'TodoListWritten', items: [{ content: 'check', status: 'in_progress' }] },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('ChildThreadStopped');
    expect(exchanges[1].steps).toHaveLength(0);
    expect(exchanges[0].steps.map((s) => s.event.type)).toEqual(['ToolCalled', 'TodoListWritten']);
  });
});

describe('a stopped child and the attention badge', () => {
  it('counts an idle stopped child as needing attention', () => {
    const { map, id } = makeThread('child', 'idle');
    const thread = map.get(id)!;
    thread.meta.section = 'inbox';
    thread.meta.parentThreadId = 'parent';
    expect(threadNeedsAttention(thread)).toBe(false);

    thread.meta.isStoppedChild = true;
    expect(threadNeedsAttention(thread)).toBe(true);
  });
});
