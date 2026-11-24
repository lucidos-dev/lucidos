import { beforeEach, describe, expect, it } from 'vitest';
import { getExchanges, getLabel, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeStatus, isTurnlessBoundary, type ThreadEvent } from '../thread-events';
import { threadIsBlocked } from '../store';

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
    expect(threadIsBlocked(thread)).toBe(false);

    thread.meta.isStoppedChild = true;
    expect(threadIsBlocked(thread)).toBe(true);
  });
});

describe('work held for hardening and the attention badge', () => {
  it('counts a resting thread whose work waits on hardening as blocked', () => {
    const { map, id } = makeThread('held', 'idle');
    const thread = map.get(id)!;
    thread.meta.section = 'inbox';
    thread.meta.channel = 'claude_code';
    thread.meta.codingAgentChangeState = { kind: 'unproposed', reason: 'turn_incomplete' };
    expect(threadIsBlocked(thread)).toBe(false);

    thread.meta.codingAgentChangeState = { kind: 'unproposed', reason: 'hardening_missing' };
    expect(threadIsBlocked(thread)).toBe(true);

    thread.meta.liveEventWaitCount = 1;
    expect(threadIsBlocked(thread)).toBe(false);
  });
});

describe('a ChildReportFiled note on the parent', () => {
  const filedNote = {
    type: 'ChildReportFiled',
    child_thread_id: CHILD_ID,
    child_thread_title: 'Check the venue',
    status: 'success',
    summary: 'It opens at nine.',
  } as ThreadEvent;

  it('is a turnless boundary that reads as done', () => {
    expect(isTurnlessBoundary(filedNote)).toBe(true);
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'spawn a child for the venue' },
      { type: 'ResponseGenerated', text: 'Spawned it.' },
      filedNote,
    ] as ThreadEvent[]);
    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('ChildReportFiled');
    expect(exchangeStatus(exchanges[1], '', true, false, false, true)).toBe('done');
  });
});
