import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  isWorkspaceReady,
  delayedBootStatus,
  remainingRevealMs,
  startDelayedBootStatus,
  workspaceIsUnreachable,
  BOOT_SPLASH_MIN_REVEAL_MS,
  STATUS_DELAY_MS,
} from './useBootSplashReady';
import { connectionStatus } from '../store/store';
import { revealBootEscape, setBootStatus } from '../utils/bootSplash';

// The status line is the unit under test; the splash DOM controller is not.
vi.mock('../utils/bootSplash', () => ({
  setBootStatus: vi.fn(),
  bootSplashPresent: vi.fn(() => true),
  bootSplashRevealSkipped: vi.fn(() => false),
  dismissBootSplash: vi.fn(),
  revealBootEscape: vi.fn(() => true),
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

// The message and the affordance must agree about when the workspace is
// unreachable, so both read the same predicate.
describe('workspaceIsUnreachable', () => {
  it('is true only for a direct port whose probe actually failed', () => {
    expect(workspaceIsUnreachable('disconnected', true)).toBe(true);
  });

  it('is false behind the gateway, which lazy-starts the engine itself', () => {
    expect(workspaceIsUnreachable('disconnected', false)).toBe(false);
  });

  it('is false while the first probe is still in flight, and once connected', () => {
    expect(workspaceIsUnreachable('connecting', true)).toBe(false);
    expect(workspaceIsUnreachable('connected', true)).toBe(false);
  });

  it('matches the state the status line names a cause for', () => {
    for (const connection of ['connected', 'connecting', 'disconnected'] as const) {
      for (const isDirect of [true, false]) {
        expect(delayedBootStatus(connection, isDirect) === 'Workspace not started').toBe(
          workspaceIsUnreachable(connection, isDirect),
        );
      }
    }
  });
});

describe('remainingRevealMs', () => {
  it('holds the splash for the rest of the reveal when it is still playing', () => {
    expect(remainingRevealMs(200, false)).toBe(BOOT_SPLASH_MIN_REVEAL_MS - 200);
  });

  it('holds nothing once the reveal has already had its time', () => {
    expect(remainingRevealMs(BOOT_SPLASH_MIN_REVEAL_MS + 500, false)).toBe(0);
  });

  // The gateway handover: the mark was built on the gateway splash and this
  // document only carries it, so there is no reveal to wait out. Holding the
  // floor would add a second of formed-mark stare to an already slow cold boot.
  it('holds nothing when the mark arrived already formed from the gateway splash', () => {
    expect(remainingRevealMs(0, true)).toBe(0);
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

  // Naming the cause is not enough on a direct port: the user cannot start the
  // workspace from that origin, so the splash must also offer the gateway.
  it('offers the gateway escape exactly when the workspace is unreachable', () => {
    const revealed = vi.mocked(revealBootEscape);
    revealed.mockClear();
    teardown = startDelayedBootStatus(true);
    vi.advanceTimersByTime(STATUS_DELAY_MS);
    // Still probing: no failure to act on yet.
    expect(revealed).not.toHaveBeenCalled();
    connectionStatus.value = 'disconnected';
    expect(revealed).toHaveBeenCalled();
  });

  it('never offers the escape behind the gateway, which starts the engine itself', () => {
    const revealed = vi.mocked(revealBootEscape);
    revealed.mockClear();
    teardown = startDelayedBootStatus(false);
    vi.advanceTimersByTime(STATUS_DELAY_MS);
    connectionStatus.value = 'disconnected';
    expect(revealed).not.toHaveBeenCalled();
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
