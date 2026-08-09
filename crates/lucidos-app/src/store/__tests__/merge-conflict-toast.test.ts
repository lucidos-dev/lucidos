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
import { focusThread } from '../actions/threads';

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

  it('taps through to the conflict event, not just to its thread', () => {
    // The banner announces the conflict panel, so a plain focus would land the
    // reader at the thread's saved scroll with nothing about the conflict on
    // screen. MergeConflictDetected starts its own exchange, so its own id is
    // what the turn stamps as data-event-id.
    seedThread('thread-A', 'Convert Filter Icon to Close Button');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event_id: 'mcd-1',
      event: { type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] },
      created: '2026-01-01T00:00:00Z',
    });

    toasts.value.find(t => t.key === 'merge-conflict-thread-A-c-1')!.onClick!();
    expect(focusThread).toHaveBeenCalledWith('thread-A', { targetEventId: 'mcd-1' });
  });

  it('degrades to a plain focus when the frame carries no event id', () => {
    seedThread('thread-A', 'Some Thread');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] },
      created: '2026-01-01T00:00:00Z',
    });

    toasts.value.find(t => t.key === 'merge-conflict-thread-A-c-1')!.onClick!();
    expect(focusThread).toHaveBeenCalledWith('thread-A', { targetEventId: null });
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

describe('MergeConflictDetected toast resolution', () => {
  beforeEach(() => {
    toasts.value = [];
    focusedThreadId.value = null;
    threadMap.value = new Map();
    vi.clearAllMocks();
  });

  function raiseConflict(threadId: string, changeId: string): void {
    handleThreadEvent({
      thread_id: threadId,
      seq: 1,
      event_id: `mcd-${changeId}`,
      event: { type: 'MergeConflictDetected', change_id: changeId, files: ['x.rs'] },
      created: '2026-01-01T00:00:00Z',
    });
  }

  it('updates the same toast to "resolved" when the conflict change is applied', () => {
    seedThread('thread-A', 'Explaining Git Worktree Tear-Downs');
    raiseConflict('thread-A', 'c-1');

    const before = toasts.value.find(t => t.message.toLowerCase().includes('merge conflict'));
    expect(before!.message).toContain('resolving automatically');
    expect(before!.type).toBe('warning');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event: { type: 'ChangeApplied', change_id: 'c-1' },
      created: '2026-01-01T00:00:10Z',
    });

    // Same toast (same key → same id), now success + "resolved".
    const after = toasts.value.find(t => t.key === 'merge-conflict-thread-A-c-1');
    expect(after).toBeTruthy();
    expect(after!.id).toBe(before!.id);
    expect(after!.message).toContain('resolved');
    expect(after!.message).not.toContain('resolving automatically');
    expect(after!.type).toBe('success');
  });

  it('keeps the deep link when the toast transitions to "resolved"', () => {
    // One toast the reader watched change its wording, so its tap must keep
    // going to the same place.
    seedThread('thread-A', 'Convert Filter Icon to Close Button');
    raiseConflict('thread-A', 'c-1');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event: { type: 'ChangeApplied', change_id: 'c-1' },
      created: '2026-01-01T00:00:10Z',
    });

    toasts.value.find(t => t.key === 'merge-conflict-thread-A-c-1')!.onClick!();
    expect(focusThread).toHaveBeenCalledWith('thread-A', { targetEventId: 'mcd-c-1' });
  });

  it('resolves to the NEWEST conflict panel when a cascade emitted two', () => {
    // Tier-2 then Tier-3 both emit for the same change, each opening its own
    // panel. The later one is the panel carrying the resolution.
    //
    // The middle event arrives LAST, as a backfill landing behind a live SSE
    // one: `thread.events` iterates in insertion order, so taking the last
    // match rather than the highest seq would send the tap backwards.
    seedThread('thread-A', 'Some Thread');
    raiseConflict('thread-A', 'c-1');
    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 3,
      event_id: 'mcd-later',
      event: { type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] },
      created: '2026-01-01T00:00:05Z',
    });
    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event_id: 'mcd-backfilled',
      event: { type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] },
      created: '2026-01-01T00:00:02Z',
    });

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 4,
      event: { type: 'ChangeApplied', change_id: 'c-1' },
      created: '2026-01-01T00:00:10Z',
    });

    toasts.value.find(t => t.key === 'merge-conflict-thread-A-c-1')!.onClick!();
    expect(focusThread).toHaveBeenCalledWith('thread-A', { targetEventId: 'mcd-later' });
  });

  it('the banner and the toast it becomes land on the same panel', () => {
    // One banner whose wording changes under the reader. If the two emits
    // ranked duplicate conflict events differently, the same toast would
    // silently change where it goes at the moment it says it is done.
    seedThread('thread-A', 'Some Thread');
    raiseConflict('thread-A', 'c-1');
    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event_id: 'mcd-later',
      event: { type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] },
      created: '2026-01-01T00:00:05Z',
    });

    toasts.value.find(t => t.key === 'merge-conflict-thread-A-c-1')!.onClick!();
    const banner = vi.mocked(focusThread).mock.lastCall;

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 3,
      event: { type: 'ChangeApplied', change_id: 'c-1' },
      created: '2026-01-01T00:00:10Z',
    });
    toasts.value.find(t => t.key === 'merge-conflict-thread-A-c-1')!.onClick!();
    const resolved = vi.mocked(focusThread).mock.lastCall;

    expect(banner).toEqual(['thread-A', { targetEventId: 'mcd-later' }]);
    expect(resolved).toEqual(banner);
  });

  it('dismisses the conflict toast when the change apply fails', () => {
    seedThread('thread-A', 'Some Thread');
    raiseConflict('thread-A', 'c-1');
    expect(toasts.value.some(t => t.key === 'merge-conflict-thread-A-c-1')).toBe(true);

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event: { type: 'ChangeApplyFailed', change_id: 'c-1', error: 'boom' },
      created: '2026-01-01T00:00:10Z',
    });

    expect(toasts.value.some(t => t.key === 'merge-conflict-thread-A-c-1')).toBe(false);
  });

  it('dismisses the conflict toast when the change is discarded', () => {
    seedThread('thread-A', 'Some Thread');
    raiseConflict('thread-A', 'c-1');
    expect(toasts.value.some(t => t.key === 'merge-conflict-thread-A-c-1')).toBe(true);

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event: { type: 'ChangeDiscarded', change_id: 'c-1' },
      created: '2026-01-01T00:00:10Z',
    });

    expect(toasts.value.some(t => t.key === 'merge-conflict-thread-A-c-1')).toBe(false);
  });

  it('does NOT spawn a spurious "resolved" toast when there was no conflict', () => {
    // A plain apply (no prior MergeConflictDetected) must not create a
    // merge-conflict banner — showToast(key) only updates an existing toast,
    // and the resolver is guarded on the toast already being present.
    seedThread('thread-A', 'Some Thread');

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'ChangeApplied', change_id: 'c-9' },
      created: '2026-01-01T00:00:00Z',
    });

    expect(toasts.value.some(t => t.message.toLowerCase().includes('merge conflict'))).toBe(false);
  });

  it('resolves only the matching change, leaving a sibling conflict toast intact', () => {
    seedThread('thread-A', 'Some Thread');
    raiseConflict('thread-A', 'c-1');
    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 2,
      event: { type: 'MergeConflictDetected', change_id: 'c-2', files: ['y.rs'] },
      created: '2026-01-01T00:00:05Z',
    });

    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 3,
      event: { type: 'ChangeApplied', change_id: 'c-1' },
      created: '2026-01-01T00:00:10Z',
    });

    const resolved = toasts.value.find(t => t.key === 'merge-conflict-thread-A-c-1');
    const stillPending = toasts.value.find(t => t.key === 'merge-conflict-thread-A-c-2');
    expect(resolved!.message).toContain('resolved');
    expect(resolved!.type).toBe('success');
    expect(stillPending!.message).toContain('resolving automatically');
    expect(stillPending!.type).toBe('warning');
  });
});
