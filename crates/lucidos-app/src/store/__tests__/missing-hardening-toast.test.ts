/**
 * Bug: When the engine auto-spawns a hardening session (MissingHardeningDetected),
 * the only quiet HTTP-response toast fired on a direct Apply click — so the
 * recovery sweep (CC ended without /harden) and the Apply All subsequent-change
 * path gave the user no system-level cue at all.
 *
 * Fix: handleThreadEvent's MissingHardeningDetected branch fires a toast
 * unconditionally, mirroring MergeConflictDetected. The in-thread initiator
 * panel is local context; the toast is the system-level "this is happening" cue.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { toasts, focusedThreadId, threadMap } from '../store';
import { makeOptimisticThreadState } from '../thread-events';

// focusThread imports React/Preact-coupled modules (scrollState, navigation)
// that aren't usable in this unit test — stub it so the toast onClick is
// testable in isolation. Matches the pattern in merge-conflict-toast.test.ts.
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

describe('MissingHardeningDetected SSE toast', () => {
  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    threadMap.value = new Map();
    vi.clearAllMocks();
  });

  it('shows a toast when the hardening thread is not focused', () => {
    seedThread('thread-A', 'Multiple Events Per Trigger Support');
    focusedThreadId.value = 'thread-B';

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MissingHardeningDetected' },
      created: '2026-01-01T00:00:00Z',
    });

    const t = toasts.value.find(t => t.message.toLowerCase().includes('hardening required'));
    expect(t).toBeTruthy();
    expect(t!.message).toContain('Multiple Events Per Trigger Support');
    expect(t!.type).toBe('warning');
    expect(t!.onClick).toBeTruthy();
  });

  it('still shows a toast when the hardening thread IS focused', () => {
    seedThread('thread-A', 'Some Thread');
    focusedThreadId.value = 'thread-A';

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MissingHardeningDetected' },
      created: '2026-01-01T00:00:00Z',
    });

    expect(toasts.value.some(t => t.message.toLowerCase().includes('hardening required'))).toBe(true);
  });

  it('falls back to the noun "thread" when the title is missing', () => {
    // No seedThread — threadMap is empty.
    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MissingHardeningDetected' },
      created: '2026-01-01T00:00:00Z',
    });

    const t = toasts.value.find(t => t.message.toLowerCase().includes('hardening required'));
    expect(t).toBeTruthy();
    expect(t!.message).toContain('Hardening required in thread');
  });

  it('collapses two emits for the same thread into one toast', () => {
    // A thread hardens one change at a time; a re-emit (e.g. recovery after the
    // apply re-entry) must refresh a single toast, not stack two banners.
    seedThread('thread-A', 'Correcting Attribution to Lucidos Agent');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MissingHardeningDetected' },
      created: '2026-01-01T00:00:00Z',
    });
    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event: { type: 'MissingHardeningDetected' },
      created: '2026-01-01T00:00:05Z',
    });

    const matches = toasts.value.filter(t => t.message.toLowerCase().includes('hardening required'));
    expect(matches).toHaveLength(1);
  });
});
