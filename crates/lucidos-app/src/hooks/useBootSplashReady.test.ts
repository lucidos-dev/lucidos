import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { isWorkspaceReady, delayedBootStatus, startDelayedBootStatus, STATUS_DELAY_MS } from './useBootSplashReady';
import { connectionStatus } from '../store/store';
import { setBootStatus } from '../utils/bootSplash';

// The status line is the unit under test; the splash DOM controller is not.
vi.mock('../utils/bootSplash', () => ({
  setBootStatus: vi.fn(),
  bootSplashPresent: vi.fn(() => true),
  dismissBootSplash: vi.fn(),
}));

describe('isWorkspaceReady', () => {
  it('is ready only when connected AND threads are loaded', () => {
    expect(isWorkspaceReady('connected', true)).toBe(true);
  });

  it('is not ready while still connecting, even once threads loaded', () => {
    expect(isWorkspaceReady('connecting', true)).toBe(false);
  });

  it('is not ready while disconnected', () => {
    expect(isWorkspaceReady('disconnected', true)).toBe(false);
  });

  it('is not ready when connected but threads have not loaded yet', () => {
    expect(isWorkspaceReady('connected', false)).toBe(false);
  });
});

describe('delayedBootStatus', () => {
  it('shows "Loading…" once connected, regardless of context', () => {
    expect(delayedBootStatus('connected', false)).toBe('Loading…');
    expect(delayedBootStatus('connected', true)).toBe('Loading…');
  });

  // The regression: an iOS PWA on a direct engine port whose first health
  // round-trip simply hadn't landed yet was told the workspace wasn't running —
  // by the very page that engine had just served. No probe has failed while the
  // status is 'connecting', so there is no evidence to name a cause from.
  it('never accuses a workspace of being down while the first probe is still in flight', () => {
    expect(delayedBootStatus('connecting', true)).toBe('Connecting…');
    expect(delayedBootStatus('connecting', false)).toBe('Connecting…');
  });

  it('shows "Workspace not started" on a DIRECT engine port once a probe has actually failed', () => {
    expect(delayedBootStatus('disconnected', true)).toBe('Workspace not started');
  });

  it('keeps "Connecting…" behind the gateway (which lazy-starts the engine)', () => {
    expect(delayedBootStatus('disconnected', false)).toBe('Connecting…');
  });
});

describe('startDelayedBootStatus', () => {
  const written = vi.mocked(setBootStatus);
  let teardown: (() => void) | null = null;

  beforeEach(() => {
    vi.useFakeTimers();
    written.mockClear();
    connectionStatus.value = 'connecting';
  });

  afterEach(() => {
    teardown?.();
    teardown = null;
    vi.useRealTimers();
    connectionStatus.value = 'connecting';
  });

  it('writes nothing before the delay — a normal launch never swaps the baked text', () => {
    teardown = startDelayedBootStatus(true);
    vi.advanceTimersByTime(STATUS_DELAY_MS - 1);
    expect(written).not.toHaveBeenCalled();
  });

  it('says "Connecting…" on a direct port while the first probe is still in flight', () => {
    teardown = startDelayedBootStatus(true);
    vi.advanceTimersByTime(STATUS_DELAY_MS);
    expect(written).toHaveBeenLastCalledWith('Connecting…');
  });

  // The stale-claim half of the fix: the line used to be sampled once at the
  // delay, so a connection landing a moment later left the wrong text up for the
  // rest of the splash's life.
  it('retracts the line when the connection lands after the delay', () => {
    teardown = startDelayedBootStatus(true);
    vi.advanceTimersByTime(STATUS_DELAY_MS);
    connectionStatus.value = 'disconnected';
    expect(written).toHaveBeenLastCalledWith('Workspace not started');
    connectionStatus.value = 'connected';
    expect(written).toHaveBeenLastCalledWith('Loading…');
  });

  it('teardown stops the subscription — before the delay it never fires at all', () => {
    startDelayedBootStatus(true)();
    vi.advanceTimersByTime(STATUS_DELAY_MS * 2);
    connectionStatus.value = 'connected';
    expect(written).not.toHaveBeenCalled();
  });

  it('teardown after the delay stops further updates, and is idempotent', () => {
    const stop = startDelayedBootStatus(true);
    vi.advanceTimersByTime(STATUS_DELAY_MS);
    written.mockClear();
    stop();
    stop();
    connectionStatus.value = 'connected';
    expect(written).not.toHaveBeenCalled();
  });
});
