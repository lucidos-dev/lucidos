import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { connectionStatus, engineStartedAt } from '../store';

const mockCheckHealth = vi.fn();
vi.mock('../../api/client', () => ({
  checkHealth: (...args: any[]) => mockCheckHealth(...args),
  API_BASE: 'http://localhost:3000',
  API: 'http://localhost:3000/api/v1',
}));
vi.mock('./thread-stream', () => ({
  connectThreadEvents: vi.fn(),
  disconnectThreadEvents: vi.fn(),
  reviveThreadEvents: vi.fn(),
}));
vi.mock('./thread-loading', () => ({
  loadAllThreads: vi.fn().mockResolvedValue(undefined),
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
  loadThreadEvents: vi.fn().mockResolvedValue(undefined),
  clearThreadFetchGuards: vi.fn(),
  markLoadedThreadsStale: vi.fn(),
}));
vi.mock('./chat-changes', () => ({
  refreshChangesState: vi.fn(),
  clearRestartInFlight: vi.fn(),
  RESTART_LS_KEY: 'lucidos-restart-required',
  RESTART_FAILURE_TOAST_KEY: 'restart-required',
}));
vi.mock('./notifications', () => ({
  loadUnreadNotifications: vi.fn(),
}));
const postClientLog = vi.fn();
vi.mock('../../utils/clientLog', () => ({ postClientLog: (...a: unknown[]) => postClientLog(...a) }));

const { checkConnection, noteWorkspaceStartAccepted, START_SETTLE_MS, START_BOOT_MS } = await import('./connection');
const { connectThreadEvents } = await import('./thread-stream');

const STARTED_AT = '2026-06-09T06:00:00Z';
const loaded = {
  status: 'loaded' as const,
  data: { workspace: 'test', workspace_path: '/tmp', started_at: STARTED_AT },
};
const unreachable = { status: 'failed' as const, error: 'Load failed' };

async function settleConnected(): Promise<void> {
  mockCheckHealth.mockResolvedValueOnce(loaded);
  await checkConnection();
  expect(connectionStatus.value).toBe('connected');
}

async function failOnce(): Promise<void> {
  mockCheckHealth.mockResolvedValueOnce(unreachable);
  await checkConnection();
}

async function succeedOnce(): Promise<void> {
  mockCheckHealth.mockResolvedValueOnce(loaded);
  await checkConnection();
}

beforeEach(() => {
  vi.clearAllMocks();
  connectionStatus.value = 'connected';
  engineStartedAt.value = null;
});

describe('connection dot debounces transient health failures', () => {
  it('a single transient failure does NOT flip the dot to disconnected', async () => {
    await settleConnected();

    await failOnce();

    expect(connectionStatus.value).toBe('connected');
  });

  it('three consecutive failures stay connected (within the debounce window)', async () => {
    await settleConnected();

    await failOnce();
    await failOnce();
    await failOnce();

    expect(connectionStatus.value).toBe('connected');
  });

  it('a sustained outage (4 consecutive failures) does flip to disconnected', async () => {
    await settleConnected();

    await failOnce();
    await failOnce();
    await failOnce();
    expect(connectionStatus.value).toBe('connected');

    await failOnce();
    expect(connectionStatus.value).toBe('disconnected');
  });

  it('a success in the middle of a blip resets the failure counter', async () => {
    await settleConnected();

    await failOnce();
    await failOnce();
    await failOnce();
    await succeedOnce();
    expect(connectionStatus.value).toBe('connected');

    await failOnce();
    await failOnce();
    await failOnce();
    expect(connectionStatus.value).toBe('connected');
  });

  it('recovery from a real disconnect requires consecutive successes (symmetric hysteresis)', async () => {
    await settleConnected();

    await failOnce();
    await failOnce();
    await failOnce();
    await failOnce();
    expect(connectionStatus.value).toBe('disconnected');

    await succeedOnce();
    expect(connectionStatus.value).toBe('disconnected');

    await succeedOnce();
    expect(connectionStatus.value).toBe('connected');
  });
});

describe('a stopped workspace obeys the same debounce as a failed probe', () => {
  const stopped = { status: 'stopped' as const };

  async function stoppedOnce(): Promise<void> {
    mockCheckHealth.mockResolvedValueOnce(stopped);
    await checkConnection();
  }

  it('keeps connected through the tolerated probes', async () => {
    await settleConnected();
    await stoppedOnce();
    await stoppedOnce();
    await stoppedOnce();
    expect(connectionStatus.value).toBe('connected');
  });

  it('reads stopped, not disconnected, once the outage is sustained', async () => {
    await settleConnected();
    for (let i = 0; i < 4; i++) await stoppedOnce();
    expect(connectionStatus.value).toBe('stopped');
  });

  it('reads stopped at once from a window that was never connected', async () => {
    connectionStatus.value = 'connecting';
    await stoppedOnce();
    expect(connectionStatus.value).toBe('stopped');
  });

});

describe('an accepted Start reads as starting until the engine answers', () => {
  const stopped = { status: 'stopped' as const };
  const BOOTED_AT = '2026-06-09T07:00:00Z';
  const booted = { status: 'loaded' as const, data: { ...loaded.data, started_at: BOOTED_AT } };

  async function stoppedWindowStarts(): Promise<void> {
    await settleConnected();
    for (let i = 0; i < 4; i++) {
      mockCheckHealth.mockResolvedValueOnce(stopped);
      await checkConnection();
    }
    expect(connectionStatus.value).toBe('stopped');
    noteWorkspaceStartAccepted();
  }

  afterEach(() => {
    vi.useRealTimers();
  });

  it('says starting at once, through the booting probes, then connected', async () => {
    await stoppedWindowStarts();
    expect(connectionStatus.value).toBe('starting');

    for (let i = 0; i < 6; i++) await failOnce();
    expect(connectionStatus.value).toBe('starting');

    mockCheckHealth.mockResolvedValueOnce(booted);
    await checkConnection();
    expect(connectionStatus.value).toBe('connected');
  });

  it('a later outage is a plain disconnect again', async () => {
    await stoppedWindowStarts();
    mockCheckHealth.mockResolvedValueOnce(booted);
    await checkConnection();

    for (let i = 0; i < 4; i++) await failOnce();
    expect(connectionStatus.value).toBe('disconnected');
  });

  it('holds a stopped answer as starting only inside the settle window', async () => {
    vi.useFakeTimers();
    await stoppedWindowStarts();

    mockCheckHealth.mockResolvedValueOnce(stopped);
    await checkConnection();
    expect(connectionStatus.value).toBe('starting');

    vi.advanceTimersByTime(START_SETTLE_MS + 1);
    mockCheckHealth.mockResolvedValueOnce(stopped);
    await checkConnection();
    expect(connectionStatus.value).toBe('stopped');
  });

  it('gives up on a boot that outlasts the budget', async () => {
    vi.useFakeTimers();
    await stoppedWindowStarts();

    vi.advanceTimersByTime(START_BOOT_MS + 1);
    await failOnce();
    expect(connectionStatus.value).toBe('disconnected');
  });
});

describe('a probe the page was away for proves nothing', () => {
  function setVisibility(state: DocumentVisibilityState) {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state });
    document.dispatchEvent(new Event('visibilitychange'));
  }

  afterEach(() => {
    setVisibility('visible');
    mockCheckHealth.mockReset();
  });

  it('does not charge failures made while hidden against the tolerance', async () => {
    await settleConnected();
    setVisibility('hidden');
    for (let i = 0; i < 6; i++) await failOnce();
    expect(connectionStatus.value).toBe('connected');

    setVisibility('visible');
    await failOnce();
    await failOnce();
    await failOnce();
    expect(connectionStatus.value).toBe('connected');
    await failOnce();
    expect(connectionStatus.value).toBe('disconnected');
  });

  it('does not charge a failure that straddled a hide', async () => {
    await settleConnected();
    mockCheckHealth.mockImplementation(async () => {
      setVisibility('hidden');
      setVisibility('visible');
      return unreachable;
    });
    for (let i = 0; i < 6; i++) await checkConnection();
    expect(connectionStatus.value).toBe('connected');
  });

  it('does not reopen the event stream on a failed probe the dot is hiding', async () => {
    await settleConnected();
    vi.mocked(connectThreadEvents).mockClear();
    setVisibility('hidden');
    await failOnce();
    expect(connectionStatus.value).toBe('connected');
    expect(connectThreadEvents).not.toHaveBeenCalled();
  });

  it('reconnects on the first good probe after the page comes back', async () => {
    await settleConnected();
    for (let i = 0; i < 4; i++) await failOnce();
    expect(connectionStatus.value).toBe('disconnected');

    setVisibility('hidden');
    setVisibility('visible');
    await succeedOnce();
    expect(connectionStatus.value).toBe('connected');
  });
});

describe('overlapping checkConnection calls share one probe', () => {
  it('issues one request and hands both callers the same verdict', async () => {
    await settleConnected();
    mockCheckHealth.mockClear();
    mockCheckHealth.mockResolvedValue(loaded);

    const [a, b] = await Promise.all([checkConnection(), checkConnection()]);

    expect(mockCheckHealth).toHaveBeenCalledTimes(1);
    expect(a).toBe(true);
    expect(b).toBe(true);
  });

  it('charges one bad moment against the tolerance once, not twice', async () => {
    await settleConnected();
    mockCheckHealth.mockResolvedValue(unreachable);

    for (let i = 0; i < 3; i++) {
      await Promise.all([checkConnection(), checkConnection()]);
      expect(connectionStatus.value).toBe('connected');
    }

    await Promise.all([checkConnection(), checkConnection()]);
    expect(connectionStatus.value).toBe('disconnected');
    mockCheckHealth.mockReset();
  });

  it('does not coalesce a later call into a settled one', async () => {
    await settleConnected();
    mockCheckHealth.mockClear();
    mockCheckHealth.mockResolvedValue(loaded);

    await checkConnection();
    await checkConnection();

    expect(mockCheckHealth).toHaveBeenCalledTimes(2);
  });
});

describe('connection transitions leave a breadcrumb, steady state does not', () => {
  it('logs once going red and once coming back, and nothing in between', async () => {
    await settleConnected();
    postClientLog.mockClear();

    await failOnce();
    await failOnce();
    await failOnce();
    expect(postClientLog).not.toHaveBeenCalled();

    await failOnce();
    expect(connectionStatus.value).toBe('disconnected');
    expect(postClientLog).toHaveBeenCalledTimes(1);

    await failOnce();
    expect(postClientLog).toHaveBeenCalledTimes(1);

    await succeedOnce();
    expect(postClientLog).toHaveBeenCalledTimes(1);

    await succeedOnce();
    expect(connectionStatus.value).toBe('connected');
    expect(postClientLog).toHaveBeenCalledTimes(2);
  });

  it('logs nothing while the dot stays green across many good polls', async () => {
    await settleConnected();
    postClientLog.mockClear();

    for (let i = 0; i < 10; i++) await succeedOnce();

    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('carries the direction and the raw probe result, not just the new state', async () => {
    await settleConnected();
    postClientLog.mockClear();

    await failOnce();
    await failOnce();
    await failOnce();
    await failOnce();

    const [category, message, data] = postClientLog.mock.calls[0];
    expect(category).toBe('connection');
    expect(message).toBe('state_changed');
    expect(data).toMatchObject({ from: 'connected', to: 'disconnected', probe_ok: false });
    expect(data).not.toHaveProperty('consecutive_failures');
    expect(data).not.toHaveProperty('consecutive_successes');
  });

  it('reports how long the previous colour held, and null for the first flip', async () => {
    await settleConnected();
    postClientLog.mockClear();

    await failOnce();
    await failOnce();
    await failOnce();
    await failOnce();
    await succeedOnce();
    await succeedOnce();

    const [red, green] = postClientLog.mock.calls.map((c) => c[2]);
    expect(typeof green.held_ms).toBe('number');
    expect(green.held_ms).toBeGreaterThanOrEqual(0);
    expect(red.held_ms === null || typeof red.held_ms === 'number').toBe(true);
  });
});
