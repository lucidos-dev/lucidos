import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import { engineRestarting } from '../store';

// The engine-restart safety timeout used to live in UiBlockingOverlay's effect,
// armed only while the overlay was mounted. Now that the overlay no longer mounts
// for a restart, the timeout rides `engineRestarting` directly in store/effects.ts
// — it must still fire (~300s) if the engine never comes back, so the GET-gate
// (`awaitEngineReady`, which blocks reads on `engineRestarting`) can't hang reads
// forever. These pin that the timer arms on the false→true edge and cancels on
// the true→false edge.

const handleRestartTimeout = vi.fn();
vi.mock('../actions/connection', async () => {
  const actual = await vi.importActual<typeof import('../actions/connection')>('../actions/connection');
  return { ...actual, handleRestartTimeout: () => handleRestartTimeout() };
});

const RESTART_TIMEOUT_MS = 300_000;

beforeAll(async () => {
  // Registers the safety-timer effect (and the rest) so it's subscribed when we
  // flip `engineRestarting`.
  await import('../effects');
});

beforeEach(() => {
  vi.useFakeTimers();
  handleRestartTimeout.mockClear();
  engineRestarting.value = false; // clears any armed timer via the effect
});

afterEach(() => {
  engineRestarting.value = false;
  vi.useRealTimers();
});

describe('engine-restart safety timer', () => {
  it('fires handleRestartTimeout if the engine never returns', () => {
    engineRestarting.value = true;
    expect(handleRestartTimeout).not.toHaveBeenCalled();

    vi.advanceTimersByTime(RESTART_TIMEOUT_MS);
    expect(handleRestartTimeout).toHaveBeenCalledTimes(1);
  });

  it('cancels the timer when the restart completes (engineRestarting → false)', () => {
    engineRestarting.value = true;
    vi.advanceTimersByTime(RESTART_TIMEOUT_MS / 2);
    engineRestarting.value = false; // reconnect via started_at, or spawn-failure revert

    vi.advanceTimersByTime(RESTART_TIMEOUT_MS);
    expect(handleRestartTimeout).not.toHaveBeenCalled();
  });

  it('does not arm a second timer while already restarting', () => {
    engineRestarting.value = true;
    // A redundant re-set of the same value is a no-op for the signal, but guard
    // the idempotency contract anyway: still exactly one fire.
    engineRestarting.value = true;

    vi.advanceTimersByTime(RESTART_TIMEOUT_MS);
    expect(handleRestartTimeout).toHaveBeenCalledTimes(1);
  });
});
