import { describe, it, expect, vi, beforeEach } from 'vitest';

const rows = vi.hoisted(() => ({ present: (_id: string): boolean => false }));
const unfocusThread = vi.hoisted(() => vi.fn());

vi.mock('./threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./threads')>()),
  unfocusThread,
  visibleCandidatesAround: (id: string) => (rows.present(id) ? ['gone', 'next'] : []),
}));
vi.mock('./event-navigation', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./event-navigation')>();
  return { ...actual, openListedThread: vi.fn(actual.openListedThread) };
});
vi.mock('./compose', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./compose')>();
  return { ...actual, forgetComposeState: vi.fn(actual.forgetComposeState) };
});
vi.mock('../unsentMessages', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../unsentMessages')>();
  return { ...actual, forgetUnsentMessagesOfThread: vi.fn(actual.forgetUnsentMessagesOfThread) };
});
vi.mock('./thread-loading', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./thread-loading')>();
  return {
    ...actual,
    noteThreadDeleted: vi.fn(actual.noteThreadDeleted),
    forgetThreadEventsFailures: vi.fn(actual.forgetThreadEventsFailures),
  };
});
vi.mock('./thread-navigation', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./thread-navigation')>();
  return { ...actual, removeThreadNavEntries: vi.fn(actual.removeThreadNavEntries) };
});
vi.mock('./searchRecents', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./searchRecents')>();
  return { ...actual, pruneRecents: vi.fn(actual.pruneRecents) };
});

import { dropDeletedThreads } from './threads-drop';
import { openListedThread } from './event-navigation';
import { forgetComposeState } from './compose';
import { forgetUnsentMessagesOfThread } from '../unsentMessages';
import { noteThreadDeleted, forgetThreadEventsFailures } from './thread-loading';
import { removeThreadNavEntries } from './thread-navigation';
import { pruneRecents } from './searchRecents';
import { focusedThreadId, threadMap } from '../store';
import { makeThreadState } from './threads-test-helpers';

beforeEach(() => {
  vi.clearAllMocks();
  rows.present = (id) => threadMap.value.has(id);
  threadMap.value = new Map([
    ['gone', makeThreadState('gone')],
    ['kept', makeThreadState('kept')],
  ]);
  focusedThreadId.value = null;
});

describe('dropping a deleted thread', () => {
  it('opens the neighbour of a deleted focused thread, read before the row leaves the map', () => {
    focusedThreadId.value = 'gone';

    dropDeletedThreads(['gone']);

    expect(openListedThread).toHaveBeenCalledWith('next', { revealPane: false });
    expect(unfocusThread).not.toHaveBeenCalled();
  });

  it('runs every per-thread cleanup for each deleted thread and keeps the others', () => {
    dropDeletedThreads(['gone']);

    expect(noteThreadDeleted).toHaveBeenCalledWith('gone');
    expect(forgetComposeState).toHaveBeenCalledWith('gone');
    expect(forgetUnsentMessagesOfThread).toHaveBeenCalledWith('gone');
    expect(forgetThreadEventsFailures).toHaveBeenCalledWith('gone');
    expect(removeThreadNavEntries).toHaveBeenCalledWith('gone');
    expect(pruneRecents).toHaveBeenCalledWith('gone', 'threads');
    expect(threadMap.value.has('gone')).toBe(false);
    expect(threadMap.value.has('kept')).toBe(true);
  });
});
