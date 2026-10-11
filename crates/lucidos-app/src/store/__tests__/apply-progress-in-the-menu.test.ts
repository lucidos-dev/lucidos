/**
 * An apply's progress is told in the Lucidos menu's activity group, never in a
 * toast (ADR 0306). Its row says what the apply is doing: merging, resolving a
 * merge conflict, or hardening. Its thread link lands on the event that
 * started the phase. The apply's one toast, keyed `applying-<thread>`, is its
 * result: Applied or Failed.
 *
 * A member of a running Apply All raises no result of its own: the batch
 * summary reports it. Only its failure still toasts, because a failure is
 * never silent.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Change } from '../../api/client';
import { toasts, focusedThreadId, threadMap, changes, applyAllBatch, applyPhases } from '../store';
import { makeOptimisticThreadState } from '../thread-events';
import { makeThreadAggregate } from '../actions/threads-test-helpers';
import { focusThread } from '../actions/threads';
import { liveActivityRows } from '../actions/activityRows';

// focusThread pulls Preact-coupled modules, so a landing is checked against a
// stub. Matches apply-change-toast.test.ts.
vi.mock('../actions/threads', () => ({
  focusThread: vi.fn(),
  unfocusThread: vi.fn(),
}));

// handleThreadEvent batches signal updates on requestAnimationFrame.
vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

import { handleThreadEvent } from '../actions/thread-sync';

const KEY = 'applying-thread-A';

function seedThread(id: string, title: string): void {
  const state = makeOptimisticThreadState({ id, title, channel: 'chat', initiator: 'user', eventsLoaded: true });
  threadMap.value = new Map([[id, state]]);
}

function pendingChange(id: string, threadId: string, overrides: Partial<Change> = {}): Change {
  return {
    id,
    request_id: 'req-1',
    thread_id: threadId,
    thread_title: null,
    branch_name: 'claude-code/test',
    repo_root: '/tmp/repo',
    description: 'feat: a change',
    file_count: 1,
    files: ['a.ts'],
    requires_restart: false,
    hardened: true,
    needs_hardening: false,
    apply_ready: true,
    status: 'pending',
    created_at: '2026-05-04T00:00:00Z',
    resolved_at: null,
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
    summary: null,
    incomplete: false,
    ...overrides,
  };
}

const EVENT_CREATED = '2026-01-01T00:00:00Z';
let seq = 0;
function emit(event: Parameters<typeof handleThreadEvent>[0]['event'], eventId?: string): void {
  handleThreadEvent({ thread_id: 'thread-A', seq: ++seq, event_id: eventId, event, created: EVENT_CREATED });
}

beforeEach(() => {
  toasts.value = [];
  focusedThreadId.value = null;
  threadMap.value = new Map();
  changes.value = { status: 'loaded', data: [pendingChange('c-1', 'thread-A')] };
  applyAllBatch.value = null;
  applyPhases.value = new Map();
  seq = 0;
  vi.clearAllMocks();
});

/** The apply's row in the activity group, the one place its progress shows. */
function row() {
  return liveActivityRows().find((r) => r.key === 'apply-thread-A');
}

describe('a single-thread apply', () => {
  it('says it is resolving a merge conflict in the menu, and raises no toast', () => {
    seedThread('thread-A', 'Convert Filter Icon');

    emit({ type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] }, 'mcd-1');

    expect(row()?.label).toBe('Resolving merge conflict: Convert Filter Icon');
    expect(row()?.body).toMatchObject({
      kind: 'apply-thread',
      // The menu measures the phase's elapsed time from the event's own time.
      thread: { reading: { phase: 'resolving-conflict', eventId: 'mcd-1', startedAt: EVENT_CREATED } },
    });
    expect(toasts.value).toEqual([]);
  });

  it('says it is hardening in the menu, and raises no toast', () => {
    seedThread('thread-A', 'Some Thread');

    emit({ type: 'MissingHardeningDetected' }, 'mhd-1');

    expect(row()?.label).toBe('Hardening: Some Thread');
    expect(toasts.value).toEqual([]);
  });

  it('follows hardening, a conflict and the apply, then reports the result', () => {
    seedThread('thread-A', 'Some Thread');

    emit({ type: 'MissingHardeningDetected' }, 'mhd-1');
    emit({ type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] }, 'mcd-1');
    expect(row()?.label).toBe('Resolving merge conflict: Some Thread');
    expect(toasts.value).toEqual([]);

    emit({ type: 'ChangeApplied', change_id: 'c-1' });
    expect(row()).toBeUndefined();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].key).toBe(KEY);
    expect(toasts.value[0].type).toBe('success');
    expect(toasts.value[0].message).toMatch(/^Applied/);
  });

  it('goes back to merging when the change is proposed again after hardening', () => {
    seedThread('thread-A', 'Some Thread');

    emit({ type: 'MissingHardeningDetected' }, 'mhd-1');
    emit({ type: 'ChangeProposed', change_id: 'c-1', hardened: true });

    expect(row()?.label).toBe('Merging: Some Thread');
    expect(toasts.value).toEqual([]);
  });

  it('reports the failure', () => {
    seedThread('thread-A', 'Some Thread');

    emit({ type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] });
    emit({ type: 'ChangeApplyFailed', change_id: 'c-1', error: 'boom' });

    expect(row()).toBeUndefined();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].type).toBe('error');
    expect(toasts.value[0].message).toContain('boom');
  });

  it('leaves the menu when the change is discarded mid-apply', () => {
    seedThread('thread-A', 'Some Thread');

    emit({ type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] });
    emit({ type: 'ChangeDiscarded', change_id: 'c-1' });

    expect(row()).toBeUndefined();
    expect(toasts.value.some((x) => x.key === KEY)).toBe(false);
  });

  it('shows no row for a re-propose outside an apply', () => {
    seedThread('thread-A', 'Some Thread');

    emit({ type: 'ChangeProposed', change_id: 'c-1' });
    emit({ type: 'ChangeHardened', change_id: 'c-1' });

    expect(row()).toBeUndefined();
    expect(toasts.value).toEqual([]);
  });

  it('lands the Applied toast on the event that started the last phase', () => {
    seedThread('thread-A', 'Some Thread');

    emit({ type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] }, 'mcd-1');
    emit({ type: 'ChangeApplied', change_id: 'c-1' });

    toasts.value.find((x) => x.key === KEY)!.onClick!();
    expect(focusThread).toHaveBeenCalledWith('thread-A', { targetEventId: 'mcd-1' });
  });

  it('names no title it does not have', () => {
    handleThreadEvent({
      thread_id: 'thread-A',
      seq: 1,
      event: { type: 'MissingHardeningDetected' },
      created: '2026-01-01T00:00:00Z',
      aggregate: makeThreadAggregate('thread-A'),
    });

    expect(row()?.label).toMatch(/^Hardening: /);
  });
});

describe('an Apply All member', () => {
  beforeEach(() => {
    applyAllBatch.value = { changeIds: ['c-1', 'c-2'], resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: [] };
  });

  it('records its phase for the Apply All row and raises no toast of its own', () => {
    seedThread('thread-A', 'Some Thread');

    emit({ type: 'MergeConflictDetected', change_id: 'c-1', files: ['x.rs'] }, 'mcd-1');

    expect(toasts.value).toHaveLength(0);
    expect(applyPhases.value.get('thread-A')).toEqual({ phase: 'resolving-conflict', eventId: 'mcd-1', startedAt: EVENT_CREATED });
  });

  it('counts as resolved when it applies, with no Applied toast', () => {
    seedThread('thread-A', 'Some Thread');

    emit({ type: 'ChangeApplied', change_id: 'c-1' });

    expect(toasts.value).toHaveLength(0);
    expect(applyAllBatch.value?.resolvedChangeIds).toEqual(['c-1']);
    expect(applyPhases.value.has('thread-A')).toBe(false);
  });

  it('still toasts its failure', () => {
    seedThread('thread-A', 'Some Thread');

    emit({ type: 'ChangeApplyFailed', change_id: 'c-1', error: 'boom' });

    expect(toasts.value.find((x) => x.key === KEY)?.type).toBe('error');
    expect(applyAllBatch.value?.resolvedChangeIds).toEqual(['c-1']);
  });
});
