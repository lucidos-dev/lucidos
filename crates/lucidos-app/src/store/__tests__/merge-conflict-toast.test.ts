/**
 * Bug: When MergeConflictDetected lands via SSE, the user has no system-level
 * notification — the inline panel was the only signal. For Apply All and
 * Tier-2 recovery paths, there is also no HTTP response to surface a toast.
 *
 * Fix: handleThreadEvent's MergeConflictDetected branch fires a toast
 * unconditionally. The panel is local context (in-thread); the toast is the
 * system-level "this is happening" cue. They each pull their weight.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { toasts, focusedThreadId, threadMap } from '../store';
import { makeOptimisticThreadState } from '../thread-events';

// focusThread imports React/Preact-coupled modules (scrollState, navigation)
// that aren't usable in this unit test — stub it so the toast onClick is
// testable in isolation. Matches the pattern in apply-change-toast.test.ts.
vi.mock('../actions/threads', () => ({
  focusThread: vi.fn(),
  unfocusThread: vi.fn(),
}));

// handleThreadEvent uses requestAnimationFrame for batched signal updates.
vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

import { handleThreadEvent } from '../actions/thread-sync';

function seedThread(id: string, title: string): void {
  const state = makeOptimisticThreadState({
    id,
    title,
    channel: 'chat',
    initiator: 'user',
    eventsLoaded: true,
  });
  threadMap.value = new Map([[id, state]]);
}

describe('MergeConflictDetected SSE toast', () => {
  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    threadMap.value = new Map();
    vi.clearAllMocks();
  });

  it('shows a toast when the conflict thread is not focused', () => {
    seedThread('thread-A', 'Multiple Events Per Trigger Support');
    focusedThreadId.value = 'thread-B';

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] },
      created: '2026-01-01T00:00:00Z',
    });

    const t = toasts.value.find(t => t.message.toLowerCase().includes('merge conflict'));
    expect(t).toBeTruthy();
    expect(t!.message).toContain('Multiple Events Per Trigger Support');
    expect(t!.type).toBe('warning');
    expect(t!.onClick).toBeTruthy();
  });

  it('still shows a toast when the conflict thread IS focused', () => {
    seedThread('thread-A', 'Some Thread');
    focusedThreadId.value = 'thread-A';

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] },
      created: '2026-01-01T00:00:00Z',
    });

    expect(toasts.value.some(t => t.message.toLowerCase().includes('merge conflict'))).toBe(true);
  });

  it('falls back to the noun "thread" when the title is missing', () => {
    // No seedThread — threadMap is empty.
    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] },
      created: '2026-01-01T00:00:00Z',
    });

    const t = toasts.value.find(t => t.message.toLowerCase().includes('merge conflict'));
    expect(t).toBeTruthy();
    expect(t!.message).toContain('Merge conflict in thread');
  });

  it('collapses two emits for the same thread+change into one toast', () => {
    // Tier-2 → Tier-3 cascade emits MergeConflictDetected twice for the same
    // change (each opens its own initiator-panel exchange in the thread).
    // The toast is a system-level cue and should fire once, not twice.
    seedThread('thread-A', 'Correcting Attribution to Lucidos Agent');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] },
      created: '2026-01-01T00:00:00Z',
    });
    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event: { type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] },
      created: '2026-01-01T00:00:05Z',
    });

    const matches = toasts.value.filter(t => t.message.toLowerCase().includes('merge conflict'));
    expect(matches).toHaveLength(1);
  });

  it('emits a separate toast for a different change in the same thread', () => {
    seedThread('thread-A', 'Some Thread');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] },
      created: '2026-01-01T00:00:00Z',
    });
    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event: { type: 'MergeConflictDetected', change_id: 'c-2', files: ['y.rs'] },
      created: '2026-01-01T00:00:05Z',
    });

    const matches = toasts.value.filter(t => t.message.toLowerCase().includes('merge conflict'));
    expect(matches).toHaveLength(2);
  });
});
