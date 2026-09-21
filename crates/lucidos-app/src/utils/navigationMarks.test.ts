import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  markNavigationStart,
  navigationClass,
  reportNavigation,
  takeNavigationStart,
  _resetNavigationMarksForTesting,
} from './navigationMarks';
import {
  flushPerfQueue,
  _resetPerfQueueForTesting,
  _setPerfEnabledForTesting,
} from './perfQueue';

/** Every sample the queue flushed, read off the batched POST body. */
function flushed(fetchMock: ReturnType<typeof vi.fn>): Array<Record<string, unknown>> {
  flushPerfQueue();
  const calls = fetchMock.mock.calls;
  if (calls.length === 0) return [];
  return calls.flatMap((call) => JSON.parse((call[1] as { body: string }).body));
}

describe('navigationClass: a view key reduced to something loggable', () => {
  it('keeps the leading segment, which says which KIND of view it is', () => {
    expect(navigationClass('settings:backup')).toBe('settings');
    expect(navigationClass('app')).toBe('app');
  });

  it('drops a file path, which a view key can carry and a log must not', () => {
    expect(navigationClass('file:/Users/me/notes/private.md')).toBe('file');
  });

  it('drops a digest, so a draft email subject cannot reach the log', () => {
    expect(navigationClass('form:email-confirm:9f2ac401')).toBe('form');
  });

  it('answers `none` for no view, rather than an empty string', () => {
    expect(navigationClass(null)).toBe('none');
    expect(navigationClass('')).toBe('none');
  });
});

describe('the mark is taken exactly once', () => {
  beforeEach(() => {
    _resetNavigationMarksForTesting();
    _setPerfEnabledForTesting(true);
  });

  afterEach(() => _setPerfEnabledForTesting(null));

  it('hands the mark over and then has none', () => {
    markNavigationStart('pane', 'content', 100);
    expect(takeNavigationStart()).toEqual({ kind: 'pane', to: 'content', start: 100 });
    expect(takeNavigationStart()).toBeUndefined();
  });

  it('keeps only the LATEST, since a replaced navigation never painted', () => {
    markNavigationStart('pane', 'content', 100);
    markNavigationStart('content', 'settings', 200);
    expect(takeNavigationStart()?.start).toBe(200);
  });

  it('stamps nothing at all while recording is off', () => {
    _setPerfEnabledForTesting(false);
    markNavigationStart('pane', 'content', 100);
    expect(takeNavigationStart()).toBeUndefined();
  });
});

describe('reportNavigation', () => {
  let fetchMock: ReturnType<typeof vi.fn>;
  let rafs: Array<() => void>;

  beforeEach(() => {
    _resetNavigationMarksForTesting();
    _resetPerfQueueForTesting();
    rafs = [];
    vi.stubGlobal('requestAnimationFrame', (cb: () => void) => { rafs.push(cb); return 1; });
    fetchMock = vi.fn(() => Promise.resolve({ ok: true } as Response));
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    _setPerfEnabledForTesting(null);
    _resetPerfQueueForTesting();
    vi.unstubAllGlobals();
  });

  it('records the span once the frame the user sees is about to paint', () => {
    _setPerfEnabledForTesting(true);
    markNavigationStart('content', 'content-pane', performance.now());
    reportNavigation('content-view', 'settings');
    // Nothing is recorded until the rAF resolves: the span is intent to PAINT.
    expect(flushed(fetchMock)).toEqual([]);
    rafs.forEach((cb) => cb());
    const samples = flushed(fetchMock);
    expect(samples).toHaveLength(1);
    expect(samples[0]).toMatchObject({
      category: 'perf',
      message: 'navigation',
      data: { kind: 'content', to: 'settings', firedBy: 'content-view' },
    });
    expect((samples[0].data as { ms: number }).ms).toBeGreaterThanOrEqual(0);
  });

  it('falls back to the stamp\'s own destination when the fire point names none', () => {
    _setPerfEnabledForTesting(true);
    markNavigationStart('pane', 'content', performance.now());
    reportNavigation('pane');
    rafs.forEach((cb) => cb());
    expect((flushed(fetchMock)[0].data as { to: string }).to).toBe('content');
  });

  it('fires for the FIRST taker only, so one navigation is one sample', () => {
    _setPerfEnabledForTesting(true);
    markNavigationStart('pane', 'content', performance.now());
    reportNavigation('pane');
    // The second fire point runs on the same navigation and finds nothing.
    reportNavigation('content-view', 'settings');
    rafs.forEach((cb) => cb());
    expect(flushed(fetchMock)).toHaveLength(1);
  });

  it('discards a STALE mark rather than reporting an idle stretch', () => {
    // The backstop. A stamp whose fire point never runs would sit there. The
    // next navigation's effect would then take it and report the idle gap as
    // its own span, which is worse than no sample: a plausible number nobody
    // can trace.
    _setPerfEnabledForTesting(true);
    markNavigationStart('pane', 'content', performance.now() - 60_000);
    reportNavigation('content-view', 'settings');
    expect(rafs).toHaveLength(0);
    expect(flushed(fetchMock)).toEqual([]);
  });

  it('still reports a genuinely slow navigation, under the stale bound', () => {
    _setPerfEnabledForTesting(true);
    markNavigationStart('content', 'content-pane', performance.now() - 900);
    reportNavigation('content-view', 'settings');
    rafs.forEach((cb) => cb());
    const ms = (flushed(fetchMock)[0].data as { ms: number }).ms;
    expect(ms).toBeGreaterThanOrEqual(900);
  });

  it('drops the sample when the frame was suspended by a backgrounded page', () => {
    // A hidden page suspends animation frames for as long as it is hidden, and
    // iOS backgrounds a PWA constantly. The callback then runs on resume, so
    // the span would be the suspension rather than the navigation.
    _setPerfEnabledForTesting(true);
    let clock = 10_000;
    vi.stubGlobal('performance', { now: () => clock });
    markNavigationStart('pane', 'content', clock);
    reportNavigation('pane');
    expect(rafs).toHaveLength(1);

    clock += 45_000; // backgrounded, then resumed
    rafs.forEach((cb) => cb());
    expect(flushed(fetchMock)).toEqual([]);
  });

  it('drops a navigation superseded before its frame ran', () => {
    // Two taps inside one frame. Preact commits twice before any animation
    // frame runs, so both reports are scheduled. The first names a destination
    // that was never painted, and one painted frame owes one sample.
    _setPerfEnabledForTesting(true);
    markNavigationStart('pane', 'content', performance.now());
    reportNavigation('pane');
    markNavigationStart('pane', 'thread', performance.now());
    reportNavigation('pane');
    expect(rafs).toHaveLength(2);

    rafs.forEach((cb) => cb());
    const samples = flushed(fetchMock);
    expect(samples).toHaveLength(1);
    expect((samples[0].data as { to: string }).to).toBe('thread');
  });

  it('refuses a mark whose expected view is not the one that arrived', () => {
    // A desktop reveal that expands a collapsed pane onto the view it already
    // holds changes no key, so no effect runs and the mark waits. Without the
    // expectation, an unrelated change soon after would be reported from that
    // reveal's start time, inflated by the gap between them.
    _setPerfEnabledForTesting(true);
    markNavigationStart('content', 'content-pane', performance.now(), 'app');
    reportNavigation('content-view', 'settings');
    expect(rafs).toHaveLength(0);
    expect(flushed(fetchMock)).toEqual([]);
  });

  it('accepts the mark when the expected view is the one that arrived', () => {
    _setPerfEnabledForTesting(true);
    markNavigationStart('content', 'content-pane', performance.now(), 'settings:backup');
    reportNavigation('content-view', 'settings:backup');
    rafs.forEach((cb) => cb());
    const samples = flushed(fetchMock);
    expect(samples).toHaveLength(1);
    expect((samples[0].data as { to: string }).to).toBe('settings');
  });

  it('lets a pane swap be taken by either fire point, expecting no view', () => {
    // `navigateToPane` names no view: on mobile a reveal swaps the pane AND may
    // change the key, and whichever effect runs first owns the reading.
    _setPerfEnabledForTesting(true);
    markNavigationStart('pane', 'content', performance.now());
    reportNavigation('content-view', 'app');
    rafs.forEach((cb) => cb());
    expect(flushed(fetchMock)).toHaveLength(1);
  });

  it('does nothing at all with no mark pending', () => {
    _setPerfEnabledForTesting(true);
    reportNavigation('pane');
    expect(rafs).toHaveLength(0);
    expect(flushed(fetchMock)).toEqual([]);
  });

  it('buffers nothing and posts nothing while recording is off', () => {
    _setPerfEnabledForTesting(false);
    markNavigationStart('pane', 'content', performance.now());
    reportNavigation('pane');
    rafs.forEach((cb) => cb());
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('survives a flush that rejects, since telemetry may not reach the caller', () => {
    _setPerfEnabledForTesting(true);
    fetchMock.mockImplementation(() => Promise.reject(new Error('offline')));
    markNavigationStart('pane', 'content', performance.now());
    reportNavigation('pane');
    expect(() => rafs.forEach((cb) => cb())).not.toThrow();
    expect(() => flushPerfQueue()).not.toThrow();
  });
});
