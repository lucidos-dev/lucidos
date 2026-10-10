import { describe, it, expect, beforeEach } from 'vitest';
import type { ApplyEstimates, Change } from '../../api/client';
import { NO_APPLY_ESTIMATES, threadMap, type ApplyPhaseReading } from '../store';
import { makeOptimisticThreadState } from '../thread-events';
import type { BackgroundActivity } from '../backgroundActivity';
import { activityRows, type ActivityRowsInput } from './activityRows';

function change(id: string, threadId: string, overrides: Partial<Change> = {}): Change {
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

const NOW = Date.parse('2026-05-04T10:00:00Z');
const TEN_MIN_AGO = '2026-05-04T09:50:00Z';
const ESTIMATES: ApplyEstimates = {
  hardening: { typical_secs: 20 * 60, runs: 5 },
  resolving_conflict: { typical_secs: 18 * 60, runs: 5 },
};

const IDLE: ActivityRowsInput = {
  activities: [],
  applyAllActive: false,
  applyAllCanceling: false,
  batch: null,
  pending: [],
  phases: new Map(),
  applyingNow: new Map(),
  estimates: NO_APPLY_ESTIMATES,
  nowMs: NOW,
};

const frontend: BackgroundActivity = {
  kind: 'frontend-refresh', label: 'Building frontend', detail: '42s', progress: null,
};
const engine: BackgroundActivity = {
  kind: 'engine-build', label: 'Building new version', detail: '2m 10s', progress: null, note: 'commits',
};

beforeEach(() => {
  threadMap.value = new Map([
    ['t-1', makeOptimisticThreadState({ id: 't-1', title: 'Card gate fix', channel: 'chat', initiator: 'user', eventsLoaded: true })],
    ['t-2', makeOptimisticThreadState({ id: 't-2', title: 'Collapse Menu', channel: 'chat', initiator: 'user', eventsLoaded: true })],
  ]);
});

describe('activityRows', () => {
  it('has no row when nothing runs', () => {
    expect(activityRows(IDLE)).toEqual([]);
  });

  it('gives each background activity one row that unfolds to it', () => {
    const rows = activityRows({ ...IDLE, activities: [engine, frontend] });
    expect(rows.map((r) => [r.label, r.detail])).toEqual([
      ['Building new version', '2m 10s'],
      ['Building frontend', '42s'],
    ]);
    expect(rows[1].body).toEqual({ kind: 'background', activity: frontend });
  });

  it('marks a build waiting for a build slot as queued, and nothing else', () => {
    const queued: BackgroundActivity = { ...engine, label: 'New version queued', queued: 'All 3 build slots are busy' };
    const rows = activityRows({ ...IDLE, activities: [queued, frontend] });
    expect(rows.map((r) => [r.label, r.queued])).toEqual([
      ['New version queued', true],
      ['Building frontend', false],
    ]);
  });

  it('names the Apply All member in flight, its phase and its position', () => {
    const pending = [change('c-1', 't-1'), change('c-2', 't-2', { needs_hardening: true })];
    const phases = new Map<string, ApplyPhaseReading>([['t-2', { phase: 'hardening', eventId: 'e-9', startedAt: null }]]);
    const rows = activityRows({
      ...IDLE,
      applyAllActive: true,
      batch: { changeIds: ['c-0', 'c-2', 'c-1'], resolvedChangeIds: ['c-0'], applyingChangeIds: [], resolvingChangeIds: [] },
      pending,
      phases,
    });
    expect(rows).toHaveLength(1);
    // The batch is the title; its member in flight, phase and all, is the body's.
    expect(rows[0].label).toBe('Applying 3 changes');
    expect(rows[0].detail).toBe('2 of 3');
    expect(rows[0].body).toEqual({
      kind: 'apply-all',
      thread: {
        threadId: 't-2',
        changeId: 'c-2',
        reading: { phase: 'hardening', eventId: 'e-9', startedAt: null },
        title: 'Collapse Menu',
        phase: 'Hardening',
      },
      position: { index: 2, total: 3 },
      // The member in flight owns its own span, so no member reads as an empty track.
      progress: { done: 1 / 3, working: 1 / 3 },
      timeLeft: null,
      canceling: false,
    });
  });

  it('says how long each phase runs and how long the batch has left, once estimates exist', () => {
    const pending = [
      change('c-2', 't-2', { needs_hardening: true, predicted_conflict: 'clean' }),
      change('c-3', 't-1', { predicted_conflict: 'conflict' }),
    ];
    const rows = activityRows({
      ...IDLE,
      applyAllActive: true,
      batch: { changeIds: ['c-2', 'c-3'], resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: [] },
      pending,
      phases: new Map([['t-2', { phase: 'hardening', eventId: null, startedAt: TEN_MIN_AGO }]]),
      estimates: ESTIMATES,
    });
    // 10 min left of a 20 min hardening, then one predicted 18 min conflict.
    expect(rows[0].body).toMatchObject({
      kind: 'apply-all',
      thread: { phase: 'Hardening · 10 min, usually ~20 min' },
      progress: { done: 0, working: 0.5 },
      timeLeft: 'about 28 min until all are applied',
    });
  });

  it('still shows an Apply All row before its membership is known', () => {
    const rows = activityRows({ ...IDLE, applyAllActive: true });
    expect(rows.map((r) => r.label)).toEqual(['Applying changes']);
    expect(rows[0].body).toEqual({
      kind: 'apply-all', thread: null, position: null, progress: null, timeLeft: null, canceling: false,
    });
  });

  it('counts one change in the singular', () => {
    const rows = activityRows({
      ...IDLE,
      applyAllActive: true,
      batch: { changeIds: ['c-1'], resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: [] },
      pending: [change('c-1', 't-1')],
    });
    expect(rows[0].label).toBe('Applying 1 change');
  });

  it('says a pressed Cancel is on its way, and carries it into the body', () => {
    const rows = activityRows({ ...IDLE, applyAllActive: true, applyAllCanceling: true });
    expect(rows.map((r) => r.label)).toEqual(['Canceling apply...']);
    expect(rows[0].body).toMatchObject({ kind: 'apply-all', canceling: true });
  });

  it('gives each single-thread apply its own row, and none to a batch member', () => {
    const pending = [change('c-1', 't-1'), change('c-2', 't-2')];
    const rows = activityRows({
      ...IDLE,
      applyAllActive: true,
      batch: { changeIds: ['c-2'], resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: [] },
      pending,
      // t-2 is the batch member: its phase belongs to the Apply All row.
      phases: new Map([['t-2', { phase: 'merging', eventId: null, startedAt: null }]]),
      applyingNow: new Map([['t-1', 'applying']]),
    });
    expect(rows.map((r) => r.key)).toEqual(['apply-all', 'apply-t-1']);
    expect(rows[1].label).toBe('Merging: Card gate fix');
    expect(rows[1].body).toEqual({
      kind: 'apply-thread',
      thread: {
        threadId: 't-1',
        changeId: 'c-1',
        reading: { phase: 'merging', eventId: null, startedAt: null },
        title: 'Card gate fix',
        phase: 'Merging',
      },
    });
  });

  it('counts a Changes-panel apply that is hardening, with no Apply Now behind it', () => {
    const rows = activityRows({
      ...IDLE,
      pending: [change('c-1', 't-1', { needs_hardening: true })],
      phases: new Map([['t-1', { phase: 'hardening', eventId: 'e-1', startedAt: null }]]),
    });
    expect(rows.map((r) => r.label)).toEqual(['Hardening: Card gate fix']);
  });

  /** A reloaded page saw no MergeConflictDetected, and the iOS PWA reloads on
   *  almost every open. The served row still says the apply is resolving. */
  it('counts a conflict the engine says is resolving, with no event seen', () => {
    const rows = activityRows({
      ...IDLE,
      pending: [
        change('c-1', 't-1', { resolving_conflict: true }),
        change('c-2', 't-2', { needs_hardening: true }),
      ],
    });
    expect(rows.map((r) => r.label)).toEqual(['Resolving merge conflict: Card gate fix']);
    expect(rows[0].body).toEqual({
      kind: 'apply-thread',
      thread: {
        threadId: 't-1',
        changeId: 'c-1',
        reading: { phase: 'resolving-conflict', eventId: null, startedAt: null },
        title: 'Card gate fix',
        phase: 'Resolving merge conflict',
      },
    });
  });

  /** ADR 0314: the Apply All row names the member being applied, and each
   *  member parked on a conflict keeps its own row with its phase. */
  it('gives a parked batch member its own row beside the Apply All row', () => {
    const rows = activityRows({
      ...IDLE,
      applyAllActive: true,
      batch: { changeIds: ['c-1', 'c-2', 'c-3'], resolvedChangeIds: [], applyingChangeIds: ['c-2'], resolvingChangeIds: ['c-1'] },
      pending: [
        change('c-1', 't-1', { resolving_conflict: true }),
        change('c-2', 't-2', { needs_hardening: true }),
        change('c-3', 't-3'),
      ],
    });
    expect(rows.map((r) => r.label)).toEqual(['Applying 3 changes', 'Resolving merge conflict: Card gate fix']);
    expect(rows[0].detail).toBe('2 of 3');
    expect(rows[0].body).toMatchObject({
      kind: 'apply-all',
      thread: { threadId: 't-2', phase: 'Hardening' },
      progress: { done: 0, working: 2 / 3 },
    });
  });

  it('leaves a resolving batch member to the Apply All row', () => {
    const rows = activityRows({
      ...IDLE,
      applyAllActive: true,
      batch: { changeIds: ['c-1'], resolvedChangeIds: [], applyingChangeIds: [], resolvingChangeIds: [] },
      pending: [change('c-1', 't-1', { resolving_conflict: true })],
    });
    expect(rows.map((r) => r.key)).toEqual(['apply-all']);
  });

  it('names an Apply Now whose change has not been proposed yet', () => {
    const rows = activityRows({ ...IDLE, pending: [], applyingNow: new Map([['t-1', 'requesting']]) });
    expect(rows.map((r) => r.label)).toEqual(['Applying: Card gate fix']);
  });

  it('lists builds first, then applies', () => {
    const rows = activityRows({
      ...IDLE,
      activities: [frontend],
      applyingNow: new Map([['t-1', 'requesting']]),
    });
    expect(rows.map((r) => r.key)).toEqual(['frontend-refresh', 'apply-t-1']);
  });
});
