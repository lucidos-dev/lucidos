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
  refreshThreadEvents: vi.fn().mockResolvedValue(undefined),
  loadThreadEvents: vi.fn().mockResolvedValue(undefined),
  clearForcedRetries: vi.fn(),
}));
vi.mock('./chat-changes', () => ({
  refreshChangesState: vi.fn(),
  clearRestartInFlight: vi.fn(),
  RESTART_LS_KEY: 'restart-required',
}));
vi.mock('./notifications', () => ({
  loadUnreadNotifications: vi.fn(),
}));

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
