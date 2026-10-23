/**
 * A change that leaves the landing lane without landing gets only the thread
 * event `ChangeUnqueued`. Its apply has ended, so its row must stop reading
 * "Apply...". It raises no toast of its own: `LaneBatchFailed` reports a
 * failed batch and `ChangeApplyFailed` a failed landing, and the other
 * reasons are the user's own action.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { toasts, threadMap, applyingChangeIds, applyAllBatch, applyPhases, busyChangeIds } from '../store';
import { makeOptimisticThreadState } from '../thread-events';
import type { UnqueueReason } from '../../generated/thread-event-wire';

vi.mock('../actions/threads', () => ({
  focusThread: vi.fn(),
  unfocusThread: vi.fn(),
}));

// handleThreadEvent batches signal updates on requestAnimationFrame.
vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

import { handleThreadEvent } from '../actions/thread-sync';

let seq = 0;
function emit(event: Parameters<typeof handleThreadEvent>[0]['event']): void {
  handleThreadEvent({ thread_id: 'thread-A', seq: ++seq, event, created: '2026-01-01T00:00:00Z' });
}

beforeEach(() => {
  seq = 0;
  toasts.value = [];
  const state = makeOptimisticThreadState({ id: 'thread-A', title: 'Some Thread', channel: 'chat', initiator: 'user', eventsLoaded: true });
  threadMap.value = new Map([['thread-A', state]]);
  applyingChangeIds.value = new Set(['c-1', 'unrelated']);
  applyAllBatch.value = null;
  applyPhases.value = new Map([['thread-A', { phase: 'merging', eventId: null, startedAt: null }]]);
});

describe('ChangeUnqueued', () => {
  const reasons: UnqueueReason[] = ['batch_failed', 'landing_failed', 'removed', 'set_aside', 'discarded'];

  it.each(reasons)('ends the apply for reason %s, with no toast', (reason) => {
    emit({ type: 'ChangeUnqueued', change_id: 'c-1', reason });

    expect(applyingChangeIds.value.has('c-1')).toBe(false);
    expect(applyingChangeIds.value.has('unrelated')).toBe(true);
    expect(applyPhases.value.has('thread-A')).toBe(false);
    expect(busyChangeIds.value.has('c-1')).toBe(false);
    expect(toasts.value).toEqual([]);
  });

  it('counts an Apply All member as resolved', () => {
    applyAllBatch.value = { changeIds: ['c-1', 'c-2'], resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: [] };

    emit({ type: 'ChangeUnqueued', change_id: 'c-1', reason: 'batch_failed' });

    expect(applyAllBatch.value?.resolvedChangeIds).toEqual(['c-1']);
    expect(busyChangeIds.value.has('c-1')).toBe(false);
    expect(busyChangeIds.value.has('c-2')).toBe(true);
  });
});
