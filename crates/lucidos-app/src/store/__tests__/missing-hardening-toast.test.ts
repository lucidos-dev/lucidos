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

  it('transitions the banner to "applied" once the change applies after hardening', () => {
    // The change applies automatically once hardening finishes, so a
    // ChangeApplied for this thread is the "done" signal — the sticky warning
    // must transition in place to a success "applied" toast, not keep claiming
    // hardening is still pending.
    const KEY = 'missing-hardening-thread-A';
    seedThread('thread-A', 'Run /harden-project on Lucidos project');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MissingHardeningDetected' },
      created: '2026-01-01T00:00:00Z',
    });
    expect(toasts.value.find(t => t.key === KEY)!.type).toBe('warning');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event: { type: 'ChangeApplied', change_id: 'change-1' },
      created: '2026-01-01T00:00:30Z',
    });

    const t = toasts.value.find(t => t.key === KEY);
    expect(t).toBeTruthy();
    expect(t!.message).toBe('Hardening applied for “Run /harden-project on Lucidos project”.');
    expect(t!.type).toBe('success');
    // The "hardening required" wording is gone — transitioned in place, not stacked.
    expect(toasts.value.some(t => t.message.toLowerCase().includes('hardening required'))).toBe(false);
  });

  it('does not spawn a hardening toast on a plain apply with no pending hardening', () => {
    // Guard: a normal Apply (no MissingHardeningDetected first) must not create a
    // spurious "Hardening applied" toast — the transition only refreshes an
    // existing banner.
    seedThread('thread-A', 'Some Thread');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'ChangeApplied', change_id: 'change-1' },
      created: '2026-01-01T00:00:00Z',
    });

    expect(toasts.value.some(t => t.message.toLowerCase().includes('hardening'))).toBe(false);
  });

  it('dismisses the hardening banner when the change is discarded instead', () => {
    // A discard is a terminal outcome carried by its own toast; the sticky
    // "hardening required" warning must not linger claiming it's still pending.
    const KEY = 'missing-hardening-thread-A';
    seedThread('thread-A', 'Some Thread');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MissingHardeningDetected' },
      created: '2026-01-01T00:00:00Z',
    });
    expect(toasts.value.some(t => t.key === KEY)).toBe(true);

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event: { type: 'ChangeDiscarded', change_id: 'change-1' },
      created: '2026-01-01T00:00:30Z',
    });

    expect(toasts.value.some(t => t.key === KEY)).toBe(false);
  });
});
