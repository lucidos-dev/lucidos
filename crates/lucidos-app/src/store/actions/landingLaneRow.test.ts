import { describe, it, expect, beforeEach } from 'vitest';
import type { Change, LandingLane, MainCheck } from '../../api/client';
import { EMPTY_LANDING_LANE, IDLE_MAIN_CHECK, threadMap } from '../store';
import { makeOptimisticThreadState } from '../thread-events';
import { landingLaneView, type LaneViewInput } from './landingLaneRow';

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

const thread = (id: string, title: string) =>
  [id, makeOptimisticThreadState({ id, title, channel: 'chat', initiator: 'user', eventsLoaded: true })] as const;

const LANE: LandingLane = {
  open_batch: { batch_id: 'b-1', lane_thread_id: 'lane-t', members: ['c-1', 'c-2'], step: 'resolving' },
  waiting: ['c-3'],
  gathering: false,
};
const PENDING = [change('c-1', 't-1'), change('c-2', 't-2'), change('c-3', 't-3', { thread_title: 'Fix the parser' })];
const IDLE: LaneViewInput = {
  lane: EMPTY_LANDING_LANE,
  laneCanceling: null,
  pending: PENDING,
  mainCheck: IDLE_MAIN_CHECK,
  mainCheckClosing: null,
};
const RED: MainCheck = {
  state: 'red',
  check_id: 'k-1',
  from_sha: 'aaa',
  to_sha: 'bbb',
  run_url: 'https://example.invalid/runs/1',
  fix_thread_id: 'fix-t',
  summary: 'Failed: engine.',
};

beforeEach(() => {
  threadMap.value = new Map([thread('t-1', 'Card gate fix'), thread('t-2', 'Collapse Menu')]);
});

describe('landingLaneView', () => {
  it('names the open batch, its step on the lane thread, its members and what waits', () => {
    expect(landingLaneView({ ...IDLE, lane: LANE })).toEqual({
      batch: {
        batchId: 'b-1',
        step: 'resolving',
        // The lane thread has no title until its first turn names it.
        laneThread: { threadId: 'lane-t', title: 'Lane thread' },
        members: [
          { changeId: 'c-1', threadId: 't-1', title: 'Card gate fix' },
          { changeId: 'c-2', threadId: 't-2', title: 'Collapse Menu' },
        ],
      },
      waiting: [{ changeId: 'c-3', threadId: 't-3', title: 'Fix the parser' }],
      gathering: false,
      canceling: false,
      mainCheck: null,
    });
  });

  it("carries the landing step and the lane thread's own title", () => {
    threadMap.value = new Map([...threadMap.value, thread('lane-t', 'Landing 2 changes')]);
    const lane: LandingLane = { ...LANE, open_batch: { ...LANE.open_batch!, step: 'landing' } };
    expect(landingLaneView({ ...IDLE, lane })).toMatchObject({
      batch: { step: 'landing', laneThread: { title: 'Landing 2 changes' } },
    });
  });

  it('has no batch with only waiting changes', () => {
    expect(landingLaneView({ ...IDLE, lane: { open_batch: null, waiting: ['c-3'], gathering: false } }))
      .toMatchObject({ batch: null, canceling: false });
  });

  it('carries that what waits, waits for an Apply All press', () => {
    const lane: LandingLane = { open_batch: null, waiting: ['c-3'], gathering: true };
    expect(landingLaneView({ ...IDLE, lane })).toMatchObject({ batch: null, gathering: true });
  });

  it('carries why origin holds the lane, and nothing while it does not', () => {
    const lane: LandingLane = { open_batch: null, waiting: ['c-3'], gathering: false, waiting_for_origin: 'could not push to origin: denied' };
    expect(landingLaneView({ ...IDLE, lane }).waitingForOrigin).toBe('could not push to origin: denied');
    expect(landingLaneView({ ...IDLE, lane: LANE })).not.toHaveProperty('waitingForOrigin');
  });

  it('carries a pressed Cancel for this batch only', () => {
    const canceling = (laneCanceling: string) => landingLaneView({ ...IDLE, lane: LANE, laneCanceling }).canceling;
    expect(canceling('b-1')).toBe(true);
    expect(canceling('b-other')).toBe(false);
  });

  describe('the main check line', () => {
    it('says passing', () => {
      const passed: MainCheck = { state: 'passed', check_id: 'k-1', to_sha: 'bbb' };
      expect(landingLaneView({ ...IDLE, lane: LANE, mainCheck: passed }).mainCheck).toEqual({ kind: 'passing' });
    });

    it('links a running check to its run', () => {
      const running: MainCheck = { state: 'running', check_id: 'k-1', run_url: 'https://example.invalid/runs/1' };
      expect(landingLaneView({ ...IDLE, mainCheck: running })).toEqual({
        batch: null,
        waiting: [],
        gathering: false,
        canceling: false,
        mainCheck: { kind: 'running', runUrl: 'https://example.invalid/runs/1' },
      });
    });

    it('names a red check, its fix thread and its Close', () => {
      expect(landingLaneView({ ...IDLE, mainCheck: RED }).mainCheck).toEqual({
        kind: 'red',
        checkId: 'k-1',
        summary: 'Failed: engine.',
        fixThread: { threadId: 'fix-t', title: 'Fix thread' },
        closing: false,
      });
      expect(landingLaneView({ ...IDLE, mainCheck: RED, mainCheckClosing: 'k-1' }).mainCheck)
        .toMatchObject({ closing: true });
    });

    it('draws nothing for an idle or unavailable check', () => {
      expect(landingLaneView({ ...IDLE, lane: LANE }).mainCheck).toBeNull();
      expect(landingLaneView({ ...IDLE, lane: LANE, mainCheck: { state: 'unavailable' } }).mainCheck).toBeNull();
    });
  });
});
