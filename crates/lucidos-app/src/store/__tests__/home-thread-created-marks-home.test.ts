/**
 * `HomeThreadCreated` names the home thread, and the aggregate carries no such
 * flag. So a Home born over SSE, as when another device turns its switch on,
 * must be marked home at once. Otherwise it sits in a section as an ordinary
 * thread until the next thread list read.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { threadMap, focusedThreadId } from '../store';
import { handleThreadEvent } from '../actions/thread-sync';
import { makeThreadAggregate } from '../actions/threads-test-helpers';

vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

describe('a home thread born over SSE', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });

  it('is marked home by its creation event', () => {
    handleThreadEvent({
      thread_id: 'home',
      seq: 1,
      event: { type: 'HomeThreadCreated' },
      created: '2026-04-16T12:00:00Z',
      aggregate: makeThreadAggregate('home', { channel: 'chat', title: 'Home' }),
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
  });
});
