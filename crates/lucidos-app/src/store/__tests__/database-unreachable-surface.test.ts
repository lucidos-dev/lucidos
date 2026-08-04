import { describe, it, expect, beforeEach, vi } from 'vitest';
import { connectionStatus, databaseReachable, engineRestarting, engineStartedAt, showToast, toasts, workspaceUnavailable } from '../store';
import type { HealthInfo } from '../../api/client';

// Mocked so the last block can drive `checkConnection` in isolation (same
// scaffold as restart-toast-reconnect.test.ts). Everything above it is pure and
// unaffected.
const mockCheckHealth = vi.fn();
vi.mock('../../api/client', () => ({
  checkHealth: (...args: unknown[]) => mockCheckHealth(...args),
  API_BASE: 'http://localhost:3000',
  API: 'http://localhost:3000/api/v1',
  isTransportError: () => false,
}));
vi.mock('../actions/thread-sync', () => ({
  connectThreadEvents: vi.fn(),
  disconnectThreadEvents: vi.fn(),
}));
vi.mock('../actions/thread-loading', () => ({
  loadAllThreads: vi.fn().mockResolvedValue(undefined),
  loadThreadEvents: vi.fn().mockResolvedValue(undefined),
  refreshThreadEvents: vi.fn().mockResolvedValue(undefined),
  clearForcedRetries: vi.fn(),
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

// One dead database used to produce a column of "Failed to …" toasts, one per
// startup load, none of which named the cause. The engine now states the fact
// once (`/health`'s `database_reachable`, ADR 0037) and the client renders THAT,
// suppressing the consequences behind it. These pin both halves of the bargain:
// the suppression, and the authoritative toast that is the only thing making it
// honest rather than silent.

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
    expect(databaseUnreachableMessage(false)).toContain('Docker');
  });

  it('does NOT name Docker on a packaged install, which runs its own Postgres', () => {
    // Naming a remedy that cannot apply is worse than naming none: it sends the
    // user to install something the outage has nothing to do with.
    const msg = databaseUnreachableMessage(true);
    expect(msg).not.toContain('Docker');
    expect(msg).toContain("can't reach its database");
  });
});

describe('syncDatabaseReachability', () => {
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
    // It is the only report of an outage that can outlast any timer, and it is
    // what makes suppressing everything else honest, so it must not be
    // dismissable or auto-dismissed out from under the suppression.
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
    // Ordering regression guard: `removeToast` must run BEFORE the signal clears.
    // Reversed, the retraction would be fine but a later signal-driven re-show
    // would not be; keeping this explicit pins the order the comment claims.
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
    // Only the authoritative one survives.
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
    // The claim is evidence FROM the engine. Once we cannot reach the engine we
    // can neither confirm nor refute it, and leaving it set would strand an
    // outage toast under a red dot while suppressing the engine-level toasts
    // that now own the story.
    syncDatabaseReachability(health({ database_reachable: false }));
    expect(workspaceUnavailable()).toBe(true);

    mockCheckHealth.mockResolvedValue({ status: 'failed', error: 'unreachable' });
    // The dot tolerates MAX_SUPPRESSED_FAILURES before it settles, and the
    // retirement rides the DISPLAYED status, so a tolerated blip does not flap
    // the toast. Poll past the tolerance.
    for (let i = 0; i < 5; i++) await checkConnection();

    expect(connectionStatus.value).toBe('disconnected');
    expect(databaseReachable.value).toBe(true);
    expect(toasts.value.find((t) => t.key === DATABASE_UNREACHABLE_TOAST_KEY)).toBeUndefined();
    expect(workspaceUnavailable()).toBe(false);
  });

  it('is NOT retired mid-reconnect, where a loaded probe still reads disconnected', () => {
    // The reconnect hysteresis (MIN_RECONNECT_SUCCESSES) can report
    // `disconnected` on a poll whose health actually loaded. Retiring there
    // would churn the signal false to true and back inside one tick, since the
    // fresh health is about to re-establish it. The retirement is therefore
    // gated on having no health at all, which this pins by driving the sync
    // directly with a still-unreachable payload.
    syncDatabaseReachability(health({ database_reachable: false }));
    connectionStatus.value = 'disconnected';
    syncDatabaseReachability(health({ database_reachable: false }));
    expect(databaseReachable.value).toBe(false);
    expect(toasts.value.filter((t) => t.key === DATABASE_UNREACHABLE_TOAST_KEY)).toHaveLength(1);
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
