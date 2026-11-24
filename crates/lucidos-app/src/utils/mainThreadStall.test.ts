import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { enginePackaged } from '../store/enginePackaging';
import {
  stallOf,
  installMainThreadStallProbe,
  _resetMainThreadStallForTesting,
  _stallProbeRunningForTesting,
  STALL_MS,
  TICK_MS,
} from './mainThreadStall';
import {
  flushPerfQueue,
  recordPerfSample,
  setPerfEnabled,
  _resetPerfQueueForTesting,
  _setPerfEnabledForTesting,
} from './perfQueue';

describe('stallOf: how late the tick ran', () => {
  it('reports the overshoot and calls a big one a stall', () => {
    expect(stallOf(1_400, 1_000)).toEqual({ overshootMs: 400, stalled: true });
  });

  it('stays quiet on ordinary scheduling jitter', () => {
    expect(stallOf(1_012, 1_000)).toEqual({ overshootMs: 12, stalled: false });
  });

  it('reports AT the threshold, so the bar is inclusive', () => {
    expect(stallOf(1_000 + STALL_MS, 1_000).stalled).toBe(true);
    expect(stallOf(1_000 + STALL_MS - 1, 1_000).stalled).toBe(false);
  });

  it('floors an early tick at zero, since a negative delay is not a reading', () => {
    expect(stallOf(900, 1_000)).toEqual({ overshootMs: 0, stalled: false });
  });

  it('takes a threshold from the caller, which is what makes the tick testable', () => {
    expect(stallOf(1_060, 1_000, 50).stalled).toBe(true);
  });
});

describe('the probe follows the perf gate', () => {
  let clock = 0;

  beforeEach(() => {
    vi.useFakeTimers();
    clock = 1_000;
    vi.stubGlobal('performance', { now: () => clock });
    enginePackaged.value = true;
    _resetMainThreadStallForTesting();
    _resetPerfQueueForTesting();
    _setPerfEnabledForTesting(false);
  });

  afterEach(() => {
    (document as unknown as { visibilityState: string }).visibilityState = 'visible';
    _resetMainThreadStallForTesting();
    _setPerfEnabledForTesting(null);
    enginePackaged.value = true;
    _resetPerfQueueForTesting();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('schedules NOTHING while recording is off', () => {
    installMainThreadStallProbe();
    expect(_stallProbeRunningForTesting()).toBe(false);
    vi.advanceTimersByTime(10_000);
    expect(_stallProbeRunningForTesting()).toBe(false);
  });

  it('starts the timer as soon as health identifies a from-source engine', () => {
    enginePackaged.value = null;
    _setPerfEnabledForTesting(null);
    installMainThreadStallProbe();
    expect(_stallProbeRunningForTesting()).toBe(false);

    enginePackaged.value = false;
    expect(_stallProbeRunningForTesting()).toBe(true);

    enginePackaged.value = true;
    expect(_stallProbeRunningForTesting()).toBe(false);
  });

  it('starts when the gate opens and stops when it shuts', () => {
    installMainThreadStallProbe();
    _setPerfEnabledForTesting(true);
    expect(_stallProbeRunningForTesting()).toBe(true);
    _setPerfEnabledForTesting(false);
    expect(_stallProbeRunningForTesting()).toBe(false);
  });

  it('starts at install when the flag was already on from a previous load', () => {
    _setPerfEnabledForTesting(true);
    installMainThreadStallProbe();
    expect(_stallProbeRunningForTesting()).toBe(true);
  });

  it('installs once, however many times it is called', () => {
    installMainThreadStallProbe();
    installMainThreadStallProbe();
    _setPerfEnabledForTesting(true);
    expect(_stallProbeRunningForTesting()).toBe(true);
    _setPerfEnabledForTesting(false);
    expect(_stallProbeRunningForTesting()).toBe(false);
  });

  it('stays stopped when the toggle is flipped but storage refuses the write', () => {
    _setPerfEnabledForTesting(null);
    installMainThreadStallProbe();
    const store = {
      getItem: () => null,
      setItem: () => { throw new Error('QuotaExceededError'); },
      removeItem: () => {},
    };
    vi.stubGlobal('localStorage', store);

    setPerfEnabled(true);
    expect(_stallProbeRunningForTesting()).toBe(false);
  });

  it('starts on a flag set straight in storage, the console path', () => {
    _setPerfEnabledForTesting(null);
    let stored: string | null = null;
    vi.stubGlobal('localStorage', {
      getItem: () => stored,
      setItem: (_k: string, v: string) => { stored = v; },
      removeItem: () => { stored = null; },
    });
    installMainThreadStallProbe();
    expect(_stallProbeRunningForTesting()).toBe(false);

    stored = '1';
    recordPerfSample('interaction', {});
    expect(_stallProbeRunningForTesting()).toBe(true);

    stored = null;
    recordPerfSample('interaction', {});
    expect(_stallProbeRunningForTesting()).toBe(false);
  });

  it('stops itself when the flag is removed straight from storage', () => {
    _setPerfEnabledForTesting(null);
    let stored: string | null = '1';
    vi.stubGlobal('localStorage', {
      getItem: () => stored,
      setItem: () => {},
      removeItem: () => { stored = null; },
    });
    installMainThreadStallProbe();
    expect(_stallProbeRunningForTesting()).toBe(true);

    vi.advanceTimersByTime(TICK_MS);
    expect(_stallProbeRunningForTesting()).toBe(true);

    stored = null;
    vi.advanceTimersByTime(TICK_MS);
    expect(_stallProbeRunningForTesting()).toBe(false);
  });

  function setVisibility(state: 'visible' | 'hidden'): void {
    (document as unknown as { visibilityState: string }).visibilityState = state;
    document.dispatchEvent({ type: 'visibilitychange' } as Event);
  }

  it('reports no stall for time the page spent backgrounded', () => {
    const fetchMock: ReturnType<typeof vi.fn> = vi.fn(() => Promise.resolve({ ok: true } as Response));
    vi.stubGlobal('fetch', fetchMock);
    installMainThreadStallProbe();
    _setPerfEnabledForTesting(true);

    setVisibility('hidden');
    clock += 120_000;
    vi.advanceTimersByTime(TICK_MS);
    setVisibility('visible');
    vi.advanceTimersByTime(TICK_MS);

    flushPerfQueue();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(_stallProbeRunningForTesting()).toBe(true);
  });

  it('ignores a wall-clock jump, which is not the event loop being blocked', () => {
    const fetchMock: ReturnType<typeof vi.fn> = vi.fn(() => Promise.resolve({ ok: true } as Response));
    vi.stubGlobal('fetch', fetchMock);
    installMainThreadStallProbe();
    _setPerfEnabledForTesting(true);

    vi.setSystemTime(new Date(Date.now() + 3_600_000));
    clock += TICK_MS;
    vi.advanceTimersByTime(TICK_MS);

    flushPerfQueue();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('records a stall when the tick runs late, and nothing when it does not', () => {
    const fetchMock: ReturnType<typeof vi.fn> = vi.fn(() => Promise.resolve({ ok: true } as Response));
    vi.stubGlobal('fetch', fetchMock);
    installMainThreadStallProbe();
    _setPerfEnabledForTesting(true);

    const advance = (ms: number) => { clock += ms; vi.advanceTimersByTime(ms); };

    advance(TICK_MS);
    flushPerfQueue();
    expect(fetchMock).not.toHaveBeenCalled();

    clock += 600;
    advance(TICK_MS);
    flushPerfQueue();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const body = JSON.parse((fetchMock.mock.calls[0][1] as { body: string }).body);
    expect(body[0]).toMatchObject({ category: 'perf', message: 'main-thread-stall' });
    expect(body[0].data.overshootMs).toBeGreaterThanOrEqual(600);
  });
});
