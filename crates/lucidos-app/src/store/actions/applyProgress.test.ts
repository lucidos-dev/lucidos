import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { ApplyEstimates, Change } from '../../api/client';
import { NO_APPLY_ESTIMATES, applyAllBatch, applyingNowThreadIds, applyPhases, threadMap, toasts, type ApplyPhaseReading } from '../store';
import { makeOptimisticThreadState } from '../thread-events';
import { focusThread } from './threads';
import {
  applyPhaseOf,
  batchMemberInFlight,
  batchMembersParkedBeside,
  batchSecondsLeft,
  batchSummary,
  batchTimeLeftLabel,
  formatDuration,
  phaseLabel,
  reconcileApplyProgress,
} from './applyProgress';

vi.mock('./threads', () => ({ focusThread: vi.fn() }));

function change(id: string, threadId: string | null, overrides: Partial<Change> = {}): Change {
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
    predicted_conflict: 'clean',
    ...overrides,
  };
}

const NO_PHASES = new Map<string, ApplyPhaseReading>();
const NOW = Date.parse('2026-05-04T10:00:00Z');
const MIN = 60;
/** Ten minutes before `NOW`. */
const TEN_MIN_AGO = '2026-05-04T09:50:00Z';
const ESTIMATES: ApplyEstimates = {
  hardening: { typical_secs: 20 * MIN, runs: 11 },
  resolving_conflict: { typical_secs: 18 * MIN, runs: 32 },
};

beforeEach(() => {
  threadMap.value = new Map([
    ['t-2', makeOptimisticThreadState({ id: 't-2', title: 'Collapse Menu', channel: 'chat', initiator: 'user', eventsLoaded: true })],
  ]);
  applyPhases.value = new Map();
  applyAllBatch.value = null;
  applyingNowThreadIds.value = new Map();
  toasts.value = [];
  vi.clearAllMocks();
});

describe('applyPhaseOf', () => {
  it('prefers the phase event this page saw', () => {
    const seen = { phase: 'resolving-conflict' as const, eventId: 'e-1', startedAt: TEN_MIN_AGO };
    const phases = new Map([['t-2', seen]]);
    expect(applyPhaseOf('t-2', change('c', 't-2', { needs_hardening: true }), phases)).toEqual(seen);
  });

  it('falls back to the row after a reload, start time included', () => {
    expect(applyPhaseOf('t-2', change('c', 't-2', {
      resolving_conflict: true,
      apply_phase_started_at: TEN_MIN_AGO,
    }), NO_PHASES)).toEqual({ phase: 'resolving-conflict', eventId: null, startedAt: TEN_MIN_AGO });
    expect(applyPhaseOf('t-2', change('c', 't-2', { needs_hardening: true }), NO_PHASES)?.phase).toBe('hardening');
    expect(applyPhaseOf('t-2', change('c', 't-2'), NO_PHASES)).toEqual({ phase: 'merging', eventId: null, startedAt: null });
  });

  it('claims no phase when nothing says what the apply is doing', () => {
    expect(applyPhaseOf('t-2', undefined, NO_PHASES)).toBeNull();
  });
});

describe('formatDuration', () => {
  it('reads in minutes, then hours', () => {
    expect(formatDuration(20)).toBe('under a minute');
    expect(formatDuration(6 * MIN + 40)).toBe('6 min');
    expect(formatDuration(60 * MIN)).toBe('1 h');
    expect(formatDuration(75 * MIN)).toBe('1 h 15 min');
  });
});

describe('phaseLabel', () => {
  const conflict = { phase: 'resolving-conflict' as const, eventId: null, startedAt: TEN_MIN_AGO };

  it('says how long the phase has run and how long it usually takes', () => {
    expect(phaseLabel(conflict, ESTIMATES, NOW)).toBe('Resolving merge conflict · 10 min, usually ~18 min');
  });

  // Too few runs give the engine no estimate. Elapsed time alone is still true.
  it('shows elapsed time alone without an estimate', () => {
    expect(phaseLabel(conflict, NO_APPLY_ESTIMATES, NOW)).toBe('Resolving merge conflict · 10 min');
  });

  it('adds no time to a plain merge or a phase with no start', () => {
    expect(phaseLabel({ phase: 'merging', eventId: null, startedAt: null }, ESTIMATES, NOW)).toBe('Merging');
    expect(phaseLabel({ ...conflict, startedAt: null }, ESTIMATES, NOW)).toBe('Resolving merge conflict');
    expect(phaseLabel(null, ESTIMATES, NOW)).toBe('Applying changes');
  });
});

describe('batchSecondsLeft', () => {
  const batch = { changeIds: ['c-1', 'c-2', 'c-3'], resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: [] };

  it('adds up hardening, and ends each conflict at its own time beside the queue', () => {
    const pending = [
      change('c-1', 't-1', { resolving_conflict: true, apply_phase_started_at: TEN_MIN_AGO }),
      change('c-2', 't-x', { needs_hardening: true }),
      change('c-3', 't-y', { predicted_conflict: 'conflict' }),
    ];
    // c-1 ends in 8 min. c-2 hardens 20 min, then c-3's 18 min conflict ends at 38.
    expect(batchSecondsLeft(batch, pending, NO_PHASES, ESTIMATES, NOW)).toBe((20 + 18) * MIN);
  });

  /** ADR 0314: a parked resolution runs beside the queue, so the batch ends
   *  with whichever finishes last, never with their sum. */
  it('counts a parked resolution as overlapping the queue', () => {
    const parked = { ...batch, applyingChangeIds: ['c-2'], resolvingChangeIds: ['c-1'] };
    const pending = [
      change('c-1', 't-1', { resolving_conflict: true, apply_phase_started_at: TEN_MIN_AGO }),
      change('c-2', 't-x', { needs_hardening: true }),
      change('c-3', 't-y', { needs_hardening: true }),
    ];
    // c-1 ends in 8 min, while c-2 and c-3 harden for 40 min in turn.
    expect(batchSecondsLeft(parked, pending, NO_PHASES, ESTIMATES, NOW)).toBe(40 * MIN);
    const longConflict = { ...ESTIMATES, resolving_conflict: { typical_secs: 60 * MIN, runs: 5 } };
    expect(batchSecondsLeft(parked, pending, NO_PHASES, longConflict, NOW)).toBe(50 * MIN);
  });

  it('counts a clean, hardened member as nothing', () => {
    const pending = ['c-1', 'c-2', 'c-3'].map((id) => change(id, `t-${id}`, { predicted_conflict: 'clean' }));
    expect(batchSecondsLeft(batch, pending, NO_PHASES, ESTIMATES, NOW)).toBe(0);
  });

  // A failed member stays pending, so only the batch's own record says it is done.
  it('skips a later member that already resolved out of turn', () => {
    const pending = [
      change('c-1', 't-1', { predicted_conflict: 'clean' }),
      change('c-2', 't-x', { predicted_conflict: 'clean' }),
      change('c-3', 't-y', { needs_hardening: true }),
    ];
    const resolved = { ...batch, resolvedChangeIds: ['c-3'], applyingChangeIds: [], resolvingChangeIds: [] };
    expect(batchSecondsLeft(resolved, pending, NO_PHASES, ESTIMATES, NOW)).toBe(0);
  });

  it('never counts an overrunning phase below a minute', () => {
    const pending = [
      change('c-1', 't-1', { resolving_conflict: true, apply_phase_started_at: '2026-05-04T09:00:00Z' }),
      change('c-2', 't-x'),
      change('c-3', 't-y'),
    ];
    expect(batchSecondsLeft(batch, pending, NO_PHASES, ESTIMATES, NOW)).toBe(MIN);
  });

  // A sum missing one of its terms would claim less time than the batch needs.
  it('gives no total when a needed estimate is missing', () => {
    const pending = [change('c-1', 't-1'), change('c-2', 't-x', { needs_hardening: true }), change('c-3', 't-y')];
    expect(batchSecondsLeft(batch, pending, NO_PHASES, { ...ESTIMATES, hardening: null }, NOW)).toBeNull();
    expect(batchSecondsLeft(batch, null, NO_PHASES, ESTIMATES, NOW)).toBeNull();
  });

  // An unknown prediction may hide a conflict, so it is never read as clean.
  it('gives no total when git could not predict a merge still ahead', () => {
    const pending = [change('c-1', 't-1'), change('c-2', 't-x', { predicted_conflict: 'unknown' }), change('c-3', 't-y')];
    expect(batchSecondsLeft(batch, pending, NO_PHASES, ESTIMATES, NOW)).toBeNull();
  });
});

describe('batchTimeLeftLabel', () => {
  it('says the time left, and nothing it cannot say or under a minute', () => {
    expect(batchTimeLeftLabel(28 * 60)).toBe('about 28 min until all are applied');
    expect(batchTimeLeftLabel(59)).toBeNull();
    expect(batchTimeLeftLabel(null)).toBeNull();
  });
});

describe('batchMemberInFlight', () => {
  it('names the first unresolved member and counts every resolved one', () => {
    expect(batchMemberInFlight({ changeIds: ['a', 'b', 'c'], resolvedChangeIds: ['a'], applyingChangeIds: [], resolvingChangeIds: [] }, null))
      .toEqual({ changeId: 'b', position: 2, resolved: 1, total: 3 });
    // A member that resolved out of turn still counts toward the position.
    expect(batchMemberInFlight({ changeIds: ['a', 'b', 'c'], resolvedChangeIds: ['c'], applyingChangeIds: [], resolvingChangeIds: [] }, null))
      .toEqual({ changeId: 'a', position: 2, resolved: 1, total: 3 });
    expect(batchMemberInFlight({ changeIds: ['a'], resolvedChangeIds: ['a'], applyingChangeIds: [], resolvingChangeIds: [] }, null)).toBeNull();
  });

  it('skips a member that left the pending list with no event this page saw', () => {
    // b was discarded: the engine resolves it, but only ChangeDiscarded reaches the page.
    expect(batchMemberInFlight({ changeIds: ['a', 'b', 'c'], resolvedChangeIds: ['a'], applyingChangeIds: [], resolvingChangeIds: [] }, new Set(['c'])))
      .toEqual({ changeId: 'c', position: 3, resolved: 2, total: 3 });
  });

  it('names the member the engine is applying, past a parked one', () => {
    const batch = { changeIds: ['a', 'b', 'c'], resolvedChangeIds: [], applyingChangeIds: ['b'], resolvingChangeIds: ['a'] };
    expect(batchMemberInFlight(batch, null)).toEqual({ changeId: 'b', position: 2, resolved: 0, total: 3 });
    expect(batchMembersParkedBeside(batch, null)).toEqual(['a']);
  });

  // The page resolves a member on its event before the refetch says what
  // the engine started next, so a stale applying id must not win.
  it('passes over a parked member before the engine names the next one', () => {
    const batch = { changeIds: ['a', 'b', 'c'], resolvedChangeIds: ['b'], applyingChangeIds: ['b'], resolvingChangeIds: ['a'] };
    expect(batchMemberInFlight(batch, null)).toEqual({ changeId: 'c', position: 3, resolved: 1, total: 3 });
  });

  it('names a parked member when only parked members are left', () => {
    const batch = { changeIds: ['a', 'b'], resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: ['a', 'b'] };
    expect(batchMemberInFlight(batch, null)).toEqual({ changeId: 'a', position: 2, resolved: 0, total: 2 });
    expect(batchMembersParkedBeside(batch, null)).toEqual(['b']);
  });
});

describe('batchSummary', () => {
  it('reports a clean run as success, and a partial one as info', () => {
    expect(batchSummary(5, 5)).toEqual({ message: 'Applied 5 changes', type: 'success' });
    expect(batchSummary(1, 1)).toEqual({ message: 'Applied 1 change', type: 'success' });
    expect(batchSummary(4, 5)).toEqual({ message: 'Applied 4 of 5 changes. 1 did not apply.', type: 'info' });
  });
});

/** A page suspended through an apply never saw its terminal event. The next
 *  refetch settles the phase, and reports the result the stream dropped. */
describe('reconcileApplyProgress', () => {
  const HARDENING: ApplyPhaseReading = { phase: 'hardening', eventId: 'e-7', startedAt: null };
  const PENDING = [change('c-2', 't-2')];

  it('reports an apply that landed while the page was away', () => {
    applyPhases.value = new Map([['t-2', HARDENING]]);
    reconcileApplyProgress(PENDING, [], [change('c-2', 't-2', { status: 'applied' })]);
    expect(applyPhases.value.size).toBe(0);
    const toast = toasts.value.find((t) => t.key === 'applying-t-2');
    expect(toast?.type).toBe('success');
    toast!.onClick!();
    expect(focusThread).toHaveBeenCalledWith('t-2', { targetEventId: 'e-7' });
  });

  it('keeps the phase of an apply the engine says still runs', () => {
    applyPhases.value = new Map([['t-2', HARDENING]]);
    reconcileApplyProgress(PENDING, [change('c-2', 't-2', { thread_settling: true })], []);
    expect(applyPhases.value.get('t-2')).toEqual(HARDENING);
    applyPhases.value = new Map([['t-2', HARDENING]]);
    reconcileApplyProgress(PENDING, [change('c-2', 't-2', { resolving_conflict: true })], []);
    expect(applyPhases.value.get('t-2')).toEqual(HARDENING);
    expect(toasts.value).toEqual([]);
  });

  /** A failed apply leaves its change pending on an idle thread. A phase kept
   *  then would spin the badge until a reload. */
  it('drops the phase of an apply that failed while the page was away', () => {
    applyPhases.value = new Map([['t-2', HARDENING]]);
    reconcileApplyProgress(PENDING, [change('c-2', 't-2')], []);
    expect(applyPhases.value.size).toBe(0);
    expect(toasts.value).toEqual([]);
  });

  it('drops a phase whose change went without landing, and reports nothing', () => {
    applyPhases.value = new Map([['t-2', HARDENING]]);
    reconcileApplyProgress(PENDING, [], []);
    expect(applyPhases.value.size).toBe(0);
    expect(toasts.value).toEqual([]);
  });

  /** The thread applied an older change once. A discarded one must not be
   *  reported as Applied under the older change's description. */
  it('never reports an older change of the same thread', () => {
    applyPhases.value = new Map([['t-2', HARDENING]]);
    reconcileApplyProgress(PENDING, [], [change('c-1', 't-2', { status: 'applied' })]);
    expect(toasts.value).toEqual([]);
  });

  it('leaves a batch member\'s result to the batch summary', () => {
    applyPhases.value = new Map([['t-2', HARDENING]]);
    applyAllBatch.value = { changeIds: ['c-2'], resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: [] };
    reconcileApplyProgress(PENDING, [], [change('c-2', 't-2', { status: 'applied' })]);
    expect(toasts.value).toEqual([]);
  });
});
