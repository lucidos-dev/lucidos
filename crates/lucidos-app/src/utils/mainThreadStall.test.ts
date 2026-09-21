import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  stallOf,
  installMainThreadStallProbe,
  _resetMainThreadStallForTesting,
  _stallProbeRunningForTesting,
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
    expect(stallOf(1_150, 1_000).stalled).toBe(true);
    expect(stallOf(1_149, 1_000).stalled).toBe(false);
  });

  it('floors an early tick at zero, since a negative delay is not a reading', () => {
    // A clock adjustment or a coarse timer can land the callback before its
    // deadline. That is not the main thread being fast, it is no reading.
    expect(stallOf(900, 1_000)).toEqual({ overshootMs: 0, stalled: false });
  });

  it('takes a threshold from the caller, which is what makes the tick testable', () => {
    expect(stallOf(1_060, 1_000, 50).stalled).toBe(true);
  });
});

describe('the probe follows the perf gate', () => {
  /** The probe reads the MONOTONIC clock, so a test drives this rather than
   *  `vi.setSystemTime`. Wall-clock time is exactly what it must not react to. */
  let clock = 0;

  beforeEach(() => {
    vi.useFakeTimers();
    clock = 1_000;
    vi.stubGlobal('performance', { now: () => clock });
    _resetMainThreadStallForTesting();
    _resetPerfQueueForTesting();
    _setPerfEnabledForTesting(false);
  });

  afterEach(() => {
    // The document stub is shared, so a test that fails mid-way must not leave
    // the next one running against a hidden page.
    (document as unknown as { visibilityState: string }).visibilityState = 'visible';
    _resetMainThreadStallForTesting();
    _setPerfEnabledForTesting(null);
    _resetPerfQueueForTesting();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('schedules NOTHING while recording is off', () => {
    installMainThreadStallProbe();
    expect(_stallProbeRunningForTesting()).toBe(false);
    // The whole point of default-off: not a quiet timer, no timer.
    vi.advanceTimersByTime(10_000);
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
    // A second subscription would leave a second interval behind this one.
    expect(_stallProbeRunningForTesting()).toBe(false);
  });

  it('stays stopped when the toggle is flipped but storage refuses the write', () => {
    // Blocked storage or a full quota. The toggle cannot throw, so it swallows
    // and carries on, and the gate every reader consults is still off. Starting
    // the interval here would hold a timer that can record nothing.
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
    // The module header documents `localStorage.setItem('lucidos:perf','1')`,
    // which calls nothing. The gate is read live, so ordinary samples start
    // recording anyway. The probe has to converge on that same read, or the
    // console path yields half an instrumentation.
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
    // Any gate read converges. A recorded sample is the ordinary one.
    recordPerfSample('interaction', {});
    expect(_stallProbeRunningForTesting()).toBe(true);

    stored = null;
    recordPerfSample('interaction', {});
    expect(_stallProbeRunningForTesting()).toBe(false);
  });

  it('stops itself when the flag is removed straight from storage', () => {
    // The console path in reverse, and the one the notification cannot reach:
    // a healthy tick reads no gate, so the interval would outlive recording and
    // run for nothing. An active tick is where that is noticed.
    _setPerfEnabledForTesting(null);
    let stored: string | null = '1';
    vi.stubGlobal('localStorage', {
      getItem: () => stored,
      setItem: () => {},
      removeItem: () => { stored = null; },
    });
    installMainThreadStallProbe();
    expect(_stallProbeRunningForTesting()).toBe(true);

    vi.advanceTimersByTime(250);
    expect(_stallProbeRunningForTesting()).toBe(true);

    stored = null;
    vi.advanceTimersByTime(250);
    expect(_stallProbeRunningForTesting()).toBe(false);
  });

  /** Put the page in a visibility state and fire the event, as a tab switch does. */
  function setVisibility(state: 'visible' | 'hidden'): void {
    (document as unknown as { visibilityState: string }).visibilityState = state;
    document.dispatchEvent({ type: 'visibilitychange' } as Event);
  }

  it('reports no stall for time the page spent backgrounded', () => {
    // iOS suspends a backgrounded PWA's timers outright, and backgrounds one
    // constantly. The first tick back would otherwise report the whole absence
    // as a blocked main thread, burying every real stall under fictions.
    const fetchMock: ReturnType<typeof vi.fn> = vi.fn(() => Promise.resolve({ ok: true } as Response));
    vi.stubGlobal('fetch', fetchMock);
    installMainThreadStallProbe();
    _setPerfEnabledForTesting(true);

    setVisibility('hidden');
    clock += 120_000;
    vi.advanceTimersByTime(250);
    setVisibility('visible');
    vi.advanceTimersByTime(250);

    flushPerfQueue();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(_stallProbeRunningForTesting()).toBe(true);
  });

  it('ignores a wall-clock jump, which is not the event loop being blocked', () => {
    // NTP correcting the system clock, or the user setting it. The probe reads
    // the monotonic clock, so the step is invisible to it.
    const fetchMock: ReturnType<typeof vi.fn> = vi.fn(() => Promise.resolve({ ok: true } as Response));
    vi.stubGlobal('fetch', fetchMock);
    installMainThreadStallProbe();
    _setPerfEnabledForTesting(true);

    vi.setSystemTime(new Date(Date.now() + 3_600_000));
    clock += 250; // the monotonic clock advanced by exactly one tick
    vi.advanceTimersByTime(250);

    flushPerfQueue();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('records a stall when the tick runs late, and nothing when it does not', () => {
    const fetchMock: ReturnType<typeof vi.fn> = vi.fn(() => Promise.resolve({ ok: true } as Response));
    vi.stubGlobal('fetch', fetchMock);
    installMainThreadStallProbe();
    _setPerfEnabledForTesting(true);

    // Time and the timer move together while nothing blocks the thread.
    const advance = (ms: number) => { clock += ms; vi.advanceTimersByTime(ms); };

    // A tick that arrives on time says nothing.
    advance(250);
    flushPerfQueue();
    expect(fetchMock).not.toHaveBeenCalled();

    // Now the thread is blocked: the monotonic clock runs 600ms past the next
    // deadline before the timer gets to run at all.
    clock += 600;
    advance(250);
    flushPerfQueue();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const body = JSON.parse((fetchMock.mock.calls[0][1] as { body: string }).body);
    expect(body[0]).toMatchObject({ category: 'perf', message: 'main-thread-stall' });
    expect(body[0].data.overshootMs).toBeGreaterThanOrEqual(600);
  });
});
