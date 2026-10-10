/**
 * A model call no thread made records on the home thread, and can create it
 * before any thread list did. Those calls reach the client as SSE events on a
 * thread the list never sent. The event's aggregate is what marks it home, so
 * it stays out of Current whatever event introduced it.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { threadMap, focusedThreadId } from '../store';
import { handleThreadEvent } from '../actions/thread-sync';
import { makeThreadAggregate } from '../actions/threads-test-helpers';
import { categorizeThreads } from '../../components/drawer/ThreadDrawer';

vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

function currentIds(): string[] {
  return categorizeThreads([...threadMap.value.values()]).current.map((t) => t.meta.id);
}

describe('a home thread first heard of over SSE', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });

  it('is marked home by a background model call, and stays out of Current', () => {
    handleThreadEvent({
      thread_id: 'home',
      seq: 1,
      event: { type: 'ContextCaptured' },
      created: '2026-04-16T12:00:00Z',
      aggregate: makeThreadAggregate('home', { channel: 'chat', title: 'Home', home: true }),
    });

    expect(threadMap.value.get('home')?.meta.home).toBe(true);
    expect(currentIds()).toEqual([]);
  });

  it('is marked home by its creation event', () => {
    handleThreadEvent({
      thread_id: 'home',
      seq: 1,
      event: { type: 'HomeThreadCreated' },
      created: '2026-04-16T12:00:00Z',
      aggregate: makeThreadAggregate('home', { channel: 'chat', title: 'Home', home: true }),
    });

    expect(threadMap.value.get('home')?.meta.home).toBe(true);
  });

  it('marks nothing else home', () => {
    handleThreadEvent({
      thread_id: 'other',
      seq: 1,
      event: { type: 'SessionStarted', session_id: 'cc-1' },
      created: '2026-04-16T12:00:00Z',
      aggregate: makeThreadAggregate('other', { channel: 'chat', title: 'Other' }),
    });

    expect(threadMap.value.get('other')?.meta.home).toBeUndefined();
    expect(currentIds()).toEqual(['other']);
  });
});
