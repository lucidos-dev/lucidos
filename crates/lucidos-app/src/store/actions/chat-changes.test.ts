import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Change } from '../../api/client';
import { ApiError } from '../../api/client';
import {
  changes,
  appliedChanges,
  lazyChanges,
  applyAllInProgress,
  standingApplyThreadIds,
  armingStandingApplySweep,
  toasts,
} from '../store';

const mockGetChangeById = vi.fn();
const mockApplyAll = vi.fn();
const mockArm = vi.fn();
const mockDisarm = vi.fn();
const mockDisarmAll = vi.fn();
const mockFetchChanges = vi.fn();

vi.mock(import('../../api/client'), async (importOriginal) => {
  const actual = await importOriginal();
  return {
    ...actual,
    getChangeById: (...args: Parameters<typeof actual.getChangeById>) => mockGetChangeById(...args),
    applyAllChanges: (...args: Parameters<typeof actual.applyAllChanges>) => mockApplyAll(...args),
    armStandingApply: (...args: Parameters<typeof actual.armStandingApply>) => mockArm(...args),
    disarmStandingApply: (...args: Parameters<typeof actual.disarmStandingApply>) => mockDisarm(...args),
    disarmAllStandingApplies: () => mockDisarmAll(),
    fetchChanges: (...args: Parameters<typeof actual.fetchChanges>) => mockFetchChanges(...args),
  };
});

const {
  ensureChangeLoaded,
  applyChangeSummarized,
  applyAllChanges,
  armStandingApply,
  disarmStandingApply,
  disarmAllStandingApplies,
  refreshChangesState,
} = await import('./chat-changes');

/** A request the test lands or fails by hand, so it can look mid-flight. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

function makeChange(id: string, overrides: Partial<Change> = {}): Change {
  return {
    id,
    request_id: 'req-1',
    thread_id: null,
    thread_title: null,
    branch_name: 'claude-code/test',
    repo_root: '/tmp/repo',
    description: 'chore(compose): trim docstrings',
    file_count: 5,
    files: ['a.ts', 'b.ts', 'c.ts', 'd.ts', 'e.ts'],
    requires_restart: false,
    hardened: false,
    status: 'applied',
    created_at: '2026-05-04T00:00:00Z',
    resolved_at: '2026-05-04T00:01:00Z',
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
    summary: null,
    incomplete: false,
    ...overrides,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  changes.value = { status: 'loaded', data: [] };
  appliedChanges.value = { status: 'loaded', data: [] };
  lazyChanges.value = new Map();
  applyAllInProgress.value = false;
  standingApplyThreadIds.value = new Set();
  armingStandingApplySweep.value = false;
  toasts.value = [];
});

/** The flag changes face on the press, not a round trip and an SSE frame
 *  later. The engine's answer only decides whether it stays. */
describe('the standing apply toggles on the press', () => {
  it('arms at once and keeps the flag once the engine agrees', async () => {
    const request = deferred<{ message: string }>();
    mockArm.mockReturnValue(request.promise);

    const arming = armStandingApply('t1', 'c1');
    expect(standingApplyThreadIds.value.has('t1')).toBe(true);

    request.resolve({ message: 'armed' });
    await arming;
    expect(standingApplyThreadIds.value.has('t1')).toBe(true);
  });

  it('takes the flag back when the arm fails, and says why', async () => {
    mockArm.mockRejectedValue(new ApiError(409, 'That change has already been applied or discarded'));

    await armStandingApply('t1', 'c1');

    expect(standingApplyThreadIds.value.has('t1')).toBe(false);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].type).toBe('error');
  });

  it('disarms at once', async () => {
    standingApplyThreadIds.value = new Set(['t1']);
    const request = deferred<{ message: string }>();
    mockDisarm.mockReturnValue(request.promise);

    const disarming = disarmStandingApply('t1');
    expect(standingApplyThreadIds.value.has('t1')).toBe(false);

    request.resolve({ message: 'canceled' });
    await disarming;
    expect(standingApplyThreadIds.value.has('t1')).toBe(false);
  });

  it('clears every flag at once on the workspace off, and restores them if it fails', async () => {
    standingApplyThreadIds.value = new Set(['t1', 't2']);
    const request = deferred<{ disarmed: number }>();
    mockDisarmAll.mockReturnValue(request.promise);

    const disarming = disarmAllStandingApplies();
    expect(standingApplyThreadIds.value.size).toBe(0);

    request.reject(new ApiError(500, 'database is down'));
    await disarming;
    expect([...standingApplyThreadIds.value].sort()).toEqual(['t1', 't2']);
    expect(toasts.value[0].type).toBe('error');
  });

  it('marks the sweep armed while its request is in flight', async () => {
    const request = deferred<{ batch_size: number; armed: number; message: string }>();
    mockApplyAll.mockReturnValue(request.promise);

    const sweeping = applyAllChanges(true);
    expect(armingStandingApplySweep.value).toBe(true);

    request.resolve({ batch_size: 0, armed: 2, message: 'armed' });
    await sweeping;
    expect(armingStandingApplySweep.value).toBe(false);
  });

  it('holds a cancel pressed mid-sweep until the sweep has armed', async () => {
    const sweep = deferred<{ batch_size: number; armed: number; message: string }>();
    mockApplyAll.mockReturnValue(sweep.promise);
    mockDisarmAll.mockResolvedValue({ disarmed: 2 });

    const sweeping = applyAllChanges(true);
    const canceling = disarmAllStandingApplies();
    expect(armingStandingApplySweep.value).toBe(false);
    await Promise.resolve();
    expect(mockDisarmAll).not.toHaveBeenCalled();

    sweep.resolve({ batch_size: 0, armed: 2, message: 'armed' });
    await Promise.all([sweeping, canceling]);
    expect(mockDisarmAll).toHaveBeenCalledTimes(1);
  });

  it('does not mark a plain Apply All as a sweep', async () => {
    mockApplyAll.mockReturnValue(deferred().promise);

    void applyAllChanges(false);

    expect(armingStandingApplySweep.value).toBe(false);
  });
});

/** A 404 says the engine holds no arm for the thread, which is what the owner
 *  asked for. A missed ending event is how the flag got there. */
describe('disarmStandingApply', () => {
  it('reads a 404 as already disarmed: the flag clears and nothing toasts', async () => {
    standingApplyThreadIds.value = new Set(['t1', 't2']);
    mockDisarm.mockRejectedValue(new ApiError(404, 'No standing apply on that thread'));

    await disarmStandingApply('t1');

    expect([...standingApplyThreadIds.value]).toEqual(['t2']);
    expect(toasts.value).toEqual([]);
  });

  it('still reports any other failure, and keeps the flag', async () => {
    standingApplyThreadIds.value = new Set(['t1']);
    mockDisarm.mockRejectedValue(new ApiError(500, 'database is down'));

    await disarmStandingApply('t1');

    expect(standingApplyThreadIds.value.has('t1')).toBe(true);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].type).toBe('error');
  });
});

/** "Apply all on settle" presses this and is never disabled, because it arms
 *  rather than applies and so wears no in-flight face. The single flight has to
 *  live in the action instead. */
describe('applyAllChanges', () => {
  it('drops a second press while the first is in flight', async () => {
    mockApplyAll.mockResolvedValue({ batch_size: 0, armed: 2, message: 'armed' });

    const first = applyAllChanges(true);
    await applyAllChanges(true);
    await first;

    expect(mockApplyAll).toHaveBeenCalledTimes(1);
  });

  it('takes the next press once the arm has landed', async () => {
    mockApplyAll.mockResolvedValue({ batch_size: 0, armed: 1, message: 'armed' });

    await applyAllChanges(true);
    await applyAllChanges(true);

    expect(mockApplyAll).toHaveBeenCalledTimes(2);
  });
});

describe('ensureChangeLoaded', () => {
  it('cache hit: skips fetch when change is already in appliedChanges', async () => {
    appliedChanges.value = { status: 'loaded', data: [makeChange('c-1')] };

    await ensureChangeLoaded('c-1');

    expect(mockGetChangeById).not.toHaveBeenCalled();
    expect(lazyChanges.value.has('c-1')).toBe(false);
  });

  it('cache hit: skips fetch when change is already in pending changes', async () => {
    changes.value = { status: 'loaded', data: [makeChange('c-1', { status: 'pending' })] };

    await ensureChangeLoaded('c-1');

    expect(mockGetChangeById).not.toHaveBeenCalled();
  });

  it('cache hit: skips fetch when change was already lazy-loaded', async () => {
    mockGetChangeById.mockResolvedValueOnce(makeChange('c-1'));
    await ensureChangeLoaded('c-1');
    expect(mockGetChangeById).toHaveBeenCalledTimes(1);

    await ensureChangeLoaded('c-1');
    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
  });

  it('cache miss: fetches once, populates lazyChanges so subsequent renders find desc + fileCount', async () => {
    const change = makeChange('acce637a', {
      description: 'chore(compose): trim docstrings',
      file_count: 5,
    });
    mockGetChangeById.mockResolvedValueOnce(change);

    await ensureChangeLoaded('acce637a');

    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
    expect(mockGetChangeById).toHaveBeenCalledWith('acce637a');
    const loadable = lazyChanges.value.get('acce637a');
    expect(loadable?.status).toBe('loaded');
    if (loadable?.status === 'loaded') {
      expect(loadable.data.description).toBe('chore(compose): trim docstrings');
      expect(loadable.data.file_count).toBe(5);
    }
  });

  it('dedup: two simultaneous lookups for the same id fire only one fetch', async () => {
    let resolveFetch!: (c: Change) => void;
    mockGetChangeById.mockImplementationOnce(
      () => new Promise<Change>(r => { resolveFetch = r; }),
    );

    const p1 = ensureChangeLoaded('c-1');
    const p2 = ensureChangeLoaded('c-1');

    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
    expect(lazyChanges.value.get('c-1')?.status).toBe('loading');

    resolveFetch(makeChange('c-1'));
    await Promise.all([p1, p2]);

    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
    expect(lazyChanges.value.get('c-1')?.status).toBe('loaded');
  });

  it('failed fetch: stores Loadable failed state so the body renders an error and does not refetch', async () => {
    mockGetChangeById.mockRejectedValueOnce(new Error('not found'));
    await ensureChangeLoaded('c-missing');
    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
    expect(lazyChanges.value.get('c-missing')?.status).toBe('failed');

    await ensureChangeLoaded('c-missing');
    expect(mockGetChangeById).toHaveBeenCalledTimes(1);
  });
});

// The panel refresh contract: a pull on Changes keeps both lists on screen while
// it re-reads, and its promise waits for the new state.
describe('refreshChangesState', () => {
  it('keeps the lists on screen and settles once the new state lands', async () => {
    const shown = makeChange('shown');
    changes.value = { status: 'loaded', data: [shown] };
    let land!: (state: unknown) => void;
    mockFetchChanges.mockReturnValueOnce(new Promise((res) => { land = res; }));

    let settled = false;
    const refresh = refreshChangesState().then(() => { settled = true; });
    await Promise.resolve();
    expect(changes.value).toEqual({ status: 'loaded', data: [shown] });
    expect(settled).toBe(false);

    const fresh = makeChange('fresh');
    land({ pending: [fresh], applied: [], has_more_applied: false });
    await refresh;
    expect(changes.value).toEqual({ status: 'loaded', data: [fresh] });
  });
});

/** A summary lands on a change row fetched outside the two lists, but only
 *  while it describes that row's current commit list. */
describe('a change summary reaches a lazily loaded row', () => {
  it('fills the summary of the commit list it describes', () => {
    lazyChanges.value = new Map([['c-1', { status: 'loaded', data: makeChange('c-1', { description: 'b\na' }) }]]);
    applyChangeSummarized('c-1', 'Adds a thing', 'b\na');
    const row = lazyChanges.value.get('c-1');
    expect(row?.status === 'loaded' && row.data.summary).toBe('Adds a thing');
  });

  it('drops a summary of an older commit list', () => {
    lazyChanges.value = new Map([['c-1', { status: 'loaded', data: makeChange('c-1', { description: 'c\nb\na' }) }]]);
    applyChangeSummarized('c-1', 'Stale', 'b\na');
    const row = lazyChanges.value.get('c-1');
    expect(row?.status === 'loaded' && row.data.summary).toBeNull();
  });
});
