/** The transcript's skeleton must be PAINTED before a big snapshot is applied.
 *
 *  Applying rows triggers the fold and the markdown pass in one synchronous
 *  render, and nothing paints while that runs. These cases pin the ordering the
 *  fix depends on: raise, paint, only then apply. See
 *  docs/plans/2026-08-14-thread-skeleton-covers-the-whole-open.md. */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.hoisted(() => {
  const storage = new Map<string, string>();
  (globalThis as any).localStorage = {
    getItem: (k: string) => storage.get(k) ?? null,
    setItem: (k: string, v: string) => storage.set(k, v),
    removeItem: (k: string) => storage.delete(k),
    clear: () => storage.clear(),
    get length() { return storage.size; },
    key: (_i: number) => null,
  };
  if (typeof globalThis.document === 'undefined') {
    (globalThis as any).document = {};
  }
  if (!(globalThis.document as any).querySelector) {
    (globalThis.document as any).querySelector = () => null;
  }
  if (!(globalThis.document as any).querySelectorAll) {
    (globalThis.document as any).querySelectorAll = () => [];
  }
});

import { makeThreadState } from './threads-test-helpers';
import { connectionStatus, focusedThreadId, threadMap } from '../store';
import { loadThreadEvents } from './thread-loading';
import { fetchThreadEvents } from '../../api/threads';
import { SKELETON_HOLD_EVENT_COUNT, forcedSkeletonThreadId } from '../../components/chat/threadSkeletonGate';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadById: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchThreadMessages: vi.fn(),
  saveThread: vi.fn().mockResolvedValue(undefined),
  archiveThread: vi.fn().mockResolvedValue({ archived: [] }),
}));

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
}));

const fetchEvents = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;

/** Frame callbacks the code under test has queued, flushed by hand so a test
 *  can look at the world BETWEEN the raise and the paint. */
let frames: Array<() => void> = [];

function flushFrames(): void {
  // Two nested frames per `nextPaint()`, so drain until the queue is empty.
  while (frames.length > 0) {
    const due = frames;
    frames = [];
    for (const cb of due) cb();
  }
}

/** A snapshot of `n` plain message rows, the shape `applyEventRows` folds. */
function snapshotOf(n: number): { events: unknown[]; currentAggregate: null } {
  const events = Array.from({ length: n }, (_, i) => ({
    sequence: i + 1,
    event_type: 'MessageReceived',
    payload: { text: `m${i}`, channel: 'chat' },
    created: '2026-01-01T00:00:00Z',
  }));
  return { events, currentAggregate: null };
}

/** Let the load's promise chain run up to its next frame wait. */
async function settle(): Promise<void> {
  for (let i = 0; i < 6; i++) await Promise.resolve();
}

describe('loadThreadEvents skeleton hold', () => {
  beforeEach(() => {
    frames = [];
    (globalThis as any).requestAnimationFrame = (cb: () => void) => {
      frames.push(cb);
      return frames.length;
    };
    threadMap.value = new Map();
    focusedThreadId.value = null;
    forcedSkeletonThreadId.value = null;
    connectionStatus.value = 'connected';
    fetchEvents.mockReset();
  });

  afterEach(() => {
    delete (globalThis as any).requestAnimationFrame;
  });

  it('raises the skeleton and waits for a paint before applying a big snapshot', async () => {
    threadMap.value = new Map([['t1', makeThreadState('t1')]]);
    focusedThreadId.value = 't1';
    fetchEvents.mockResolvedValue(snapshotOf(SKELETON_HOLD_EVENT_COUNT));

    const done = loadThreadEvents('t1');
    await settle();

    // Between the raise and the paint: the skeleton is up and NOT one row has
    // landed. This is the whole fix; an apply here would blank the pane.
    expect(forcedSkeletonThreadId.value).toBe('t1');
    expect(threadMap.value.get('t1')!.events.size).toBe(0);
    expect(threadMap.value.get('t1')!.eventsLoaded).toBe(false);
    expect(frames.length).toBeGreaterThan(0);

    flushFrames();
    await done;

    expect(threadMap.value.get('t1')!.events.size).toBe(SKELETON_HOLD_EVENT_COUNT);
    expect(threadMap.value.get('t1')!.eventsLoaded).toBe(true);
    expect(forcedSkeletonThreadId.value).toBe(null);
  });

  it('applies a small snapshot straight through, with no frame spent', async () => {
    threadMap.value = new Map([['t1', makeThreadState('t1')]]);
    focusedThreadId.value = 't1';
    fetchEvents.mockResolvedValue(snapshotOf(SKELETON_HOLD_EVENT_COUNT - 1));

    await loadThreadEvents('t1');

    expect(frames).toHaveLength(0);
    expect(forcedSkeletonThreadId.value).toBe(null);
    expect(threadMap.value.get('t1')!.events.size).toBe(SKELETON_HOLD_EVENT_COUNT - 1);
  });

  it('never holds a thread the user is not looking at', async () => {
    // loadAllThreads preloads every active and saved thread through here.
    threadMap.value = new Map([['t1', makeThreadState('t1')], ['t2', makeThreadState('t2')]]);
    focusedThreadId.value = 't2';
    fetchEvents.mockResolvedValue(snapshotOf(SKELETON_HOLD_EVENT_COUNT * 2));

    await loadThreadEvents('t1');

    expect(frames).toHaveLength(0);
    expect(forcedSkeletonThreadId.value).toBe(null);
    expect(threadMap.value.get('t1')!.eventsLoaded).toBe(true);
  });

  it('releases the flag when the thread leaves the map mid-hold', async () => {
    // The one path with a bare `return` between the raise and the apply. A
    // thread left shimmering here would shimmer for the life of the page.
    threadMap.value = new Map([['t1', makeThreadState('t1')]]);
    focusedThreadId.value = 't1';
    fetchEvents.mockResolvedValue(snapshotOf(SKELETON_HOLD_EVENT_COUNT));

    const done = loadThreadEvents('t1');
    await settle();
    expect(forcedSkeletonThreadId.value).toBe('t1');

    threadMap.value = new Map();
    flushFrames();
    await done;

    expect(forcedSkeletonThreadId.value).toBe(null);
  });
});
