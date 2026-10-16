import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { GatewayStatus } from '../../api/client/control';

const mocks = vi.hoisted(() => ({ getGatewayStatus: vi.fn() }));
vi.mock('../../api/client/control', () => ({ getGatewayStatus: mocks.getGatewayStatus }));

const { relayedUpdate } = await import('../store');
const { resumeRelayedUpdate, watchRelayedUpdate } = await import('./update-relay');
const { loadRelayedUpdate, saveRelayedUpdate } = await import('../updateRelay');

const MARKER = { id: 'r-1', version: '1.3.0', fromVersion: '1.2.0', since: Date.now() };

function status(current: string, relay: NonNullable<GatewayStatus['update_relay']>["request"]): GatewayStatus {
  return {
    build_id: 'b',
    update_available: false,
    release_check: {
      enabled: true,
      supported: true,
      current_version: current,
      checked_at: null,
      last_error: null,
      latest: null,
    },
    update_relay: { client: { version: current, blocker: null }, request: relay },
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  localStorage.clear();
  relayedUpdate.value = null;
  mocks.getGatewayStatus.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('watching a relayed update', () => {
  it('reports progress, then the success that ends the watch', async () => {
    const frame = { version: '1.3.0', phase: 'downloading' as const, downloaded: 1, total: 2 };
    mocks.getGatewayStatus
      .mockResolvedValueOnce(status('1.2.0', { id: 'r-1', version: '1.3.0', state: 'running', progress: frame }))
      .mockRejectedValueOnce(new Error('the service is restarting'))
      .mockResolvedValueOnce(status('1.3.0', null));
    const news = vi.fn();
    watchRelayedUpdate(MARKER, news);
    expect(loadRelayedUpdate()).toEqual(MARKER);

    await vi.advanceTimersByTimeAsync(2_000);
    expect(news).toHaveBeenLastCalledWith({ kind: 'progress', frame });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(news).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(news).toHaveBeenLastCalledWith({ kind: 'succeeded', version: '1.3.0' });

    expect(relayedUpdate.value).toBeNull();
    expect(loadRelayedUpdate()).toBeNull();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(mocks.getGatewayStatus).toHaveBeenCalledTimes(3);
  });

  it('picks a request back up after a reload, and looks at once', async () => {
    saveRelayedUpdate(MARKER);
    mocks.getGatewayStatus.mockResolvedValue(status('1.2.0', null));
    const news = vi.fn();
    resumeRelayedUpdate(news);
    await vi.advanceTimersByTimeAsync(0);
    expect(news).toHaveBeenCalledWith({ kind: 'did-not-run' });
    expect(loadRelayedUpdate()).toBeNull();
  });

  it('resumes nothing when no request was left behind', async () => {
    const news = vi.fn();
    resumeRelayedUpdate(news);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(mocks.getGatewayStatus).not.toHaveBeenCalled();
    expect(news).not.toHaveBeenCalled();
  });
});
