import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import { engineRestarting } from '../store';


const handleRestartTimeout = vi.fn();
vi.mock('../actions/connection', async () => {
  const actual = await vi.importActual<typeof import('../actions/connection')>('../actions/connection');
  return { ...actual, handleRestartTimeout: () => handleRestartTimeout() };
});

const RESTART_TIMEOUT_MS = 300_000;

beforeAll(async () => {
  await import('../effects');
});

beforeEach(() => {
  vi.useFakeTimers();
  handleRestartTimeout.mockClear();
  engineRestarting.value = false;
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
    engineRestarting.value = false;

    vi.advanceTimersByTime(RESTART_TIMEOUT_MS);
    expect(handleRestartTimeout).not.toHaveBeenCalled();
  });

  it('does not arm a second timer while already restarting', () => {
    engineRestarting.value = true;
    engineRestarting.value = true;

    vi.advanceTimersByTime(RESTART_TIMEOUT_MS);
    expect(handleRestartTimeout).toHaveBeenCalledTimes(1);
  });
});
