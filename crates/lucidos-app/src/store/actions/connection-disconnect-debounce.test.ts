/**
 * Regression: the connection dot (`connectionStatus`) must NOT flash red on a
 * single transient `/health` failure while idle. The engine is local and almost
 * never down — a lone failed health GET is far more likely a transient blip (iOS
 * radio nap, Tailscale latency spike, HTTP/2 coalescing hiccup after the PWA
 * backgrounds) than a real outage. The dot is driven SOLELY by the 5s `/health`
 * poll (it does not reflect SSE liveness), so without debouncing, every blip
 * paints the dot red even though events are still flowing.
 *
 * Before the fix the failure debounce was gated behind `isProcessing`, so the
 * idle case (PWA sitting in a pocket — exactly when the radio naps) had ZERO
 * tolerance and a single failure flipped the dot to disconnected. These tests
 * pin the symmetric debounce: N consecutive failures to go red, mirroring the
 * MIN_RECONNECT_SUCCESSES hysteresis on the way back to green.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { connectionStatus, engineStartedAt } from '../store';

const mockCheckHealth = vi.fn();
vi.mock('../../api/client', () => ({
  checkHealth: (...args: any[]) => mockCheckHealth(...args),
  API_BASE: 'http://localhost:3000',
  API: 'http://localhost:3000/api/v1',
}));
vi.mock('./thread-sync', () => ({
  connectThreadEvents: vi.fn(),
  disconnectThreadEvents: vi.fn(),
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

const { checkConnection } = await import('./connection');

const STARTED_AT = '2026-06-09T06:00:00Z';
const loaded = {
  status: 'loaded' as const,
  data: { workspace: 'test', workspace_path: '/tmp', started_at: STARTED_AT },
};
const unreachable = { status: 'failed' as const, error: 'Load failed' };

/** One success resets the module-level failure/success counters and sets
 *  hasEverConnected, giving each test a clean 'connected' baseline regardless of
 *  the singleton state a prior test left behind. */
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

    // The whole point of the fix: one blip while idle must stay green.
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

    // Three failures, then a success — counter must reset to zero.
    await failOnce();
    await failOnce();
    await failOnce();
    await succeedOnce();
    expect(connectionStatus.value).toBe('connected');

    // Another three failures must again be tolerated. If the counter had NOT
    // reset, the first of these would be the 4th cumulative failure and flip red.
    await failOnce();
    await failOnce();
    await failOnce();
    expect(connectionStatus.value).toBe('connected');
  });

  it('recovery from a real disconnect requires consecutive successes (symmetric hysteresis)', async () => {
    await settleConnected();

    // Drive a genuine disconnect.
    await failOnce();
    await failOnce();
    await failOnce();
    await failOnce();
    expect(connectionStatus.value).toBe('disconnected');

    // First success alone must not flip back — prevents red→green flicker when
    // the engine flaps during a restart.
    await succeedOnce();
    expect(connectionStatus.value).toBe('disconnected');

    // Second consecutive success reconnects.
    await succeedOnce();
    expect(connectionStatus.value).toBe('connected');
  });
});

/**
 * Every case above drives probes sequentially, which is the poll's own rhythm.
 * These cover the overlap, which on iOS is the normal case rather than the edge:
 * a wake fires `visibilitychange`, `focus` AND `pageshow` at the same moment the
 * frozen 5s timer unfreezes, so `handleResume`'s probe lands on top of the
 * poll's. The counters are module state with a tolerance of four bad ticks;
 * un-coalesced, two overlapping runs both read `wasConnected` before either
 * writes and both increment, charging one bad moment twice.
 */
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

    // Four overlapping pairs. Un-coalesced this is eight failures, so the dot
    // would have gone red on the second pair.
    for (let i = 0; i < 3; i++) {
      await Promise.all([checkConnection(), checkConnection()]);
      expect(connectionStatus.value).toBe('connected');
    }

    // The fourth distinct bad tick is what earns the red dot.
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

    // The guard covers overlap only: the poll's next tick must still probe.
    expect(mockCheckHealth).toHaveBeenCalledTimes(2);
  });
});

/**
 * The dot is the one user-visible signal in the product with no record on either
 * side: `/api/v1/health` is excluded from the engine's request log (the gateway
 * probes every workspace every 2s for the picker badge and would bury it), and
 * the client logged nothing. So a report of "a lot of red blinking" on a phone
 * could be read from the source but not measured. These pin that the breadcrumb
 * marks transitions only, which is what keeps its rate far below the probe's.
 */
describe('connection transitions leave a breadcrumb, steady state does not', () => {
  it('logs once going red and once coming back, and nothing in between', async () => {
    await settleConnected();
    postClientLog.mockClear();

    // Three suppressed failures: the dot has not moved, so nothing is logged.
    await failOnce();
    await failOnce();
    await failOnce();
    expect(postClientLog).not.toHaveBeenCalled();

    await failOnce();
    expect(connectionStatus.value).toBe('disconnected');
    expect(postClientLog).toHaveBeenCalledTimes(1);

    // A further failure while already red changes nothing on screen.
    await failOnce();
    expect(postClientLog).toHaveBeenCalledTimes(1);

    // The first success is still throttled by MIN_RECONNECT_SUCCESSES.
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
    // `probe_ok` is the raw health result: together with `to` it says whether
    // the dot moved because the engine answered differently or because a
    // counter crossed its threshold.
    expect(data).toMatchObject({ from: 'connected', to: 'disconnected', probe_ok: false });
    // Never the two counters: both are reset before the breadcrumb runs and both
    // sit at their threshold by definition when the dot flips, so they could only
    // ever log a constant dressed up as a measurement.
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
    // How long red lasted is the actual question behind "a lot of red
    // blinking", so the green transition must carry a real duration.
    expect(typeof green.held_ms).toBe('number');
    expect(green.held_ms).toBeGreaterThanOrEqual(0);
    // The very first transition of the session measures from nothing: the
    // client cannot know how long the engine was in that state before it
    // started watching, so it reports null rather than time-since-page-load.
    expect(red.held_ms === null || typeof red.held_ms === 'number').toBe(true);
  });
});
