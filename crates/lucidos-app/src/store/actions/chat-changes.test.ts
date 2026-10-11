import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Change } from '../../api/client';
import { ApiError } from '../../api/client';
import {
  changes,
  appliedChanges,
  lazyChanges,
  applyAllInProgress,
  standingApplyThreadIds,
  toasts,
} from '../store';

const mockGetChangeById = vi.fn();
const mockApplyAll = vi.fn();
const mockArm = vi.fn();
const mockDisarm = vi.fn();
const mockFetchChanges = vi.fn();

vi.mock(import('../../api/client'), async (importOriginal) => {
  const actual = await importOriginal();
  return {
    ...actual,
    getChangeById: (...args: Parameters<typeof actual.getChangeById>) => mockGetChangeById(...args),
    applyAllChanges: (...args: Parameters<typeof actual.applyAllChanges>) => mockApplyAll(...args),
    armStandingApply: (...args: Parameters<typeof actual.armStandingApply>) => mockArm(...args),
    disarmStandingApply: (...args: Parameters<typeof actual.disarmStandingApply>) => mockDisarm(...args),
    fetchChanges: (...args: Parameters<typeof actual.fetchChanges>) => mockFetchChanges(...args),
  };
});

const {
  ensureChangeLoaded,
  applyChangeSummarized,
  applyAllChanges,
  armStandingApply,
  disarmStandingApply,
  armStandingApplies,
  disarmStandingApplies,
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
    needs_hardening: true,
    apply_ready: false,
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
});

/** The Not finished section's bulk control arms and disarms only the changes
 *  it lists, one bound arm each, never the workspace-wide sweep. */
describe('a section of standing applies', () => {
  it('arms each listed change, bound to that change', async () => {
    mockArm.mockResolvedValue({ message: 'armed' });

    await armStandingApplies([
      makeChange('c1', { status: 'pending', thread_id: 't1' }),
      makeChange('c2', { status: 'pending', thread_id: 't2' }),
    ]);

    expect(mockArm.mock.calls).toEqual([['t1', 'c1'], ['t2', 'c2']]);
    expect(mockApplyAll).not.toHaveBeenCalled();
    expect([...standingApplyThreadIds.value].sort()).toEqual(['t1', 't2']);
  });

  it('disarms only the named threads', async () => {
    standingApplyThreadIds.value = new Set(['t1', 't2', 'elsewhere']);
    mockDisarm.mockResolvedValue({ message: 'canceled' });

    await disarmStandingApplies(['t1', 't2']);

    expect(mockDisarm.mock.calls).toEqual([['t1'], ['t2']]);
    expect([...standingApplyThreadIds.value]).toEqual(['elsewhere']);
  });

  // A per-thread disarm drops a tap while that thread's arm is in flight. So
  // the section's cancel waits for the section's arms, or it is lost.
  it('holds a cancel pressed mid-arm until the arms have landed', async () => {
    const slow = deferred<{ message: string }>();
    mockArm.mockResolvedValueOnce({ message: 'armed' }).mockReturnValueOnce(slow.promise);
    mockDisarm.mockResolvedValue({ message: 'canceled' });

    const arming = armStandingApplies([
      makeChange('c1', { status: 'pending', thread_id: 't1' }),
      makeChange('c2', { status: 'pending', thread_id: 't2' }),
    ]);
    const canceling = disarmStandingApplies(['t1', 't2']);
    await Promise.resolve();
    expect(mockDisarm).not.toHaveBeenCalled();

    slow.resolve({ message: 'armed' });
    await Promise.all([arming, canceling]);
    expect(mockDisarm.mock.calls).toEqual([['t1'], ['t2']]);
    expect(standingApplyThreadIds.value.size).toBe(0);
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

describe('applyAllChanges', () => {
  it('drops a second press while the first is in flight', async () => {
    const request = deferred<{ batch_size: number }>();
    mockApplyAll.mockReturnValue(request.promise);

    const first = applyAllChanges();
    await applyAllChanges();
    request.reject(new ApiError(400, 'No pending changes'));
    await first;

    expect(mockApplyAll).toHaveBeenCalledTimes(1);
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
