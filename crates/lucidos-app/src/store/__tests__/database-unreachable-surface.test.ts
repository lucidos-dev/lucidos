import { describe, it, expect, beforeEach, vi } from 'vitest';
import { effect } from '@preact/signals';
import { connectionStatus, databaseReachable, engineRestarting, engineStartedAt, showToast, toasts, workspaceUnavailable } from '../store';
import type { HealthInfo } from '../../api/client';

const mockCheckHealth = vi.fn();
vi.mock('../../api/client', () => ({
  checkHealth: (...args: unknown[]) => mockCheckHealth(...args),
  API_BASE: 'http://localhost:3000',
  API: 'http://localhost:3000/api/v1',
  isTransportError: () => false,
}));
vi.mock('../actions/thread-stream', () => ({
  connectThreadEvents: vi.fn(),
  disconnectThreadEvents: vi.fn(),
  reviveThreadEvents: vi.fn(),
}));
vi.mock('../actions/thread-loading', () => ({
  loadAllThreads: vi.fn().mockResolvedValue(undefined),
  loadThreadEvents: vi.fn().mockResolvedValue(undefined),
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
  clearThreadFetchGuards: vi.fn(),
  markLoadedThreadsStale: vi.fn(),
}));
vi.mock('../actions/notifications', () => ({
  loadUnreadNotifications: vi.fn(),
}));

const {
  DATABASE_UNREACHABLE_TOAST_KEY,
  databaseUnreachableMessage,
  syncDatabaseReachability,
  checkConnection,
} = await import('../actions/connection');


const health = (over: Partial<HealthInfo> = {}): HealthInfo => ({
  status: 'ok',
  workspace: 'dev',
  workspace_path: '/tmp/ws',
  started_at: '2026-08-04T00:00:00Z',
  ...over,
});

beforeEach(() => {
  vi.clearAllMocks();
  engineRestarting.value = false;
  databaseReachable.value = true;
  connectionStatus.value = 'connected';
  engineStartedAt.value = null;
  toasts.value = [];
});

describe('databaseUnreachableMessage', () => {
  it('names Docker on a dev install, where the database really is a container', () => {
    expect(databaseUnreachableMessage(false, false)).toContain('Docker');
  });

  it('does NOT name Docker on a packaged install, which runs its own Postgres', () => {
    const msg = databaseUnreachableMessage(true, false);
    expect(msg).not.toContain('Docker');
    expect(msg).toContain("can't reach its database");
  });

  it('points at Lucidos, never at Docker, when the database answers but the pool is used up', () => {
    for (const packaged of [false, true]) {
      const msg = databaseUnreachableMessage(packaged, true);
      expect(msg).not.toContain('Docker');
      expect(msg).toContain('database connections are busy');
      expect(msg).toContain('restart Lucidos');
    }
  });
});

describe('syncDatabaseReachability', () => {
  it('names a used-up pool in the toast when the engine reports one', () => {
    syncDatabaseReachability(health({ database_reachable: false, database_pool_exhausted: true }));
    expect(databaseReachable.value).toBe(false);
    expect(toasts.value.map(t => t.message).join(' ')).toContain('database connections are busy');
  });

  it('treats an absent field as reachable, so an older engine is unaffected', () => {
    syncDatabaseReachability(health());
    expect(databaseReachable.value).toBe(true);
    expect(toasts.value).toHaveLength(0);
  });

  it('only an explicit false claims an outage', () => {
    syncDatabaseReachability(health({ database_reachable: true }));
    expect(databaseReachable.value).toBe(true);
    expect(workspaceUnavailable()).toBe(false);
  });

  it('raises exactly one authoritative toast when the database is unreachable', () => {
    syncDatabaseReachability(health({ database_reachable: false }));
    expect(databaseReachable.value).toBe(false);
    const toast = toasts.value.find((t) => t.key === DATABASE_UNREACHABLE_TOAST_KEY);
    expect(toast).toBeTruthy();
    expect(toast?.type).toBe('error');
    expect(toast?.dismissable).toBe(false);
  });

  it('does not stack a copy per health poll', () => {
    syncDatabaseReachability(health({ database_reachable: false }));
    syncDatabaseReachability(health({ database_reachable: false }));
    syncDatabaseReachability(health({ database_reachable: false }));
    expect(toasts.value.filter((t) => t.key === DATABASE_UNREACHABLE_TOAST_KEY)).toHaveLength(1);
  });

  it('retracts the toast and reopens the UI when the database comes back', () => {
    syncDatabaseReachability(health({ database_reachable: false }));
    syncDatabaseReachability(health({ database_reachable: true }));
    expect(databaseReachable.value).toBe(true);
    expect(toasts.value.find((t) => t.key === DATABASE_UNREACHABLE_TOAST_KEY)).toBeUndefined();
  });

  it('retracts even though the retraction runs inside its own suppression window', () => {
    syncDatabaseReachability(health({ database_reachable: false }));
    expect(workspaceUnavailable()).toBe(true);
    syncDatabaseReachability(health({ database_reachable: true }));
    expect(workspaceUnavailable()).toBe(false);
    expect(toasts.value).toHaveLength(0);
  });
});

describe('toast suppression while the database is unreachable', () => {
  it('suppresses the per-load failure toasts that used to arrive twenty at a time', () => {
    syncDatabaseReachability(health({ database_reachable: false }));
    showToast('Failed to load threads', 'error');
    showToast('Failed to load apps', 'error');
    showToast('Failed to load triggers', 'error');
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].key).toBe(DATABASE_UNREACHABLE_TOAST_KEY);
  });

  it('lets an opted-in status toast through', () => {
    syncDatabaseReachability(health({ database_reachable: false }));
    showToast('Restarting engine...', 'info', { key: 'restart-required', showWhileUnavailable: true });
    expect(toasts.value.some((t) => t.message === 'Restarting engine...')).toBe(true);
  });

  it('resumes showing toasts once the database is back', () => {
    syncDatabaseReachability(health({ database_reachable: false }));
    showToast('Suppressed', 'error');
    expect(toasts.value.filter((t) => t.message === 'Suppressed')).toHaveLength(0);

    syncDatabaseReachability(health({ database_reachable: true }));
    showToast('Saved', 'success');
    expect(toasts.value.some((t) => t.message === 'Saved')).toBe(true);
  });
});

describe('a database claim never outlives the engine that made it', () => {
  it('is retired when the connection settles to disconnected', async () => {
    syncDatabaseReachability(health({ database_reachable: false }));
    expect(workspaceUnavailable()).toBe(true);

    mockCheckHealth.mockResolvedValue({ status: 'failed', error: 'unreachable' });
    for (let i = 0; i < 5; i++) await checkConnection();

    expect(connectionStatus.value).toBe('disconnected');
    expect(databaseReachable.value).toBe(true);
    expect(toasts.value.find((t) => t.key === DATABASE_UNREACHABLE_TOAST_KEY)).toBeUndefined();
    expect(workspaceUnavailable()).toBe(false);
  });

  it('is NOT retired mid-reconnect, where a loaded probe still reads disconnected', () => {
    syncDatabaseReachability(health({ database_reachable: false }));
    connectionStatus.value = 'disconnected';
    syncDatabaseReachability(health({ database_reachable: false }));
    expect(databaseReachable.value).toBe(false);
    expect(toasts.value.filter((t) => t.key === DATABASE_UNREACHABLE_TOAST_KEY)).toHaveLength(1);
  });
});

describe('the outage signal and its toast change in the order suppression needs', () => {
  it('raises the outage signal before the toast appears', () => {
    const seen: boolean[] = [];
    const dispose = effect(() => {
      if (toasts.value.some(t => t.key === DATABASE_UNREACHABLE_TOAST_KEY)) seen.push(databaseReachable.peek());
    });
    syncDatabaseReachability(health({ database_reachable: false }));
    dispose();
    expect(seen).toEqual([false]);
  });

  it('retracts the toast before the outage signal clears', () => {
    syncDatabaseReachability(health({ database_reachable: false }));
    const seen: boolean[] = [];
    const dispose = effect(() => {
      if (databaseReachable.value) seen.push(toasts.value.some(t => t.key === DATABASE_UNREACHABLE_TOAST_KEY));
    });
    syncDatabaseReachability(health({ database_reachable: true }));
    dispose();
    expect(seen).toEqual([false]);
  });

  it('does not churn the outage signal while a reconnect is still in hysteresis', async () => {
    mockCheckHealth.mockResolvedValue({ status: 'loaded', data: health() });
    await checkConnection();
    syncDatabaseReachability(health({ database_reachable: false }));
    connectionStatus.value = 'disconnected';
    const seen: boolean[] = [];
    const dispose = effect(() => {
      seen.push(databaseReachable.value);
    });
    mockCheckHealth.mockResolvedValue({ status: 'loaded', data: health({ database_reachable: false }) });
    await checkConnection();
    dispose();
    expect(seen.every(v => v === false)).toBe(true);
  });

  it('raises no engine-restarted toast on a tick that also reports an outage', async () => {
    engineStartedAt.value = '2026-08-03T00:00:00Z';
    engineRestarting.value = true;
    mockCheckHealth.mockResolvedValue({
      status: 'loaded',
      data: health({ database_reachable: false, started_at: '2026-08-04T12:00:00Z' }),
    });
    await checkConnection();
    expect(toasts.value.some(t => t.message === 'Engine restarted')).toBe(false);
  });

  it('keeps the outage toast up past any auto-dismiss window', () => {
    vi.useFakeTimers();
    try {
      syncDatabaseReachability(health({ database_reachable: false }));
      vi.advanceTimersByTime(10 * 60_000);
      expect(toasts.value.some(t => t.key === DATABASE_UNREACHABLE_TOAST_KEY)).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('workspaceUnavailable', () => {
  it('is false when the workspace can serve requests', () => {
    expect(workspaceUnavailable()).toBe(false);
  });

  it('still covers the restart window it was originally written for', () => {
    engineRestarting.value = true;
    expect(workspaceUnavailable()).toBe(true);
  });

  it('covers a dead database independently of the restart window', () => {
    databaseReachable.value = false;
    expect(workspaceUnavailable()).toBe(true);
  });
});
