// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi, type Mock } from 'vitest';
import { SLOW_LOAD_MS, frameDocumentTiming, startFrameLoadProbe, type HostSample } from './appFrameLoadProbe';

const host = (): HostSample => ({
  sinceResumeMs: 4200,
  eventStream: 'connected',
  lanes: { running: 2, queued: 1, oldestRunningMs: 800, oldestQueuedMs: 300 },
  requests: { count: 1, oldestMs: 2900, oldestPaths: ['/api/v1/threads'] },
});

const label = { app: 'thread-cost', place: 'dashboard' };

let log: Mock<(category: string, message: string, data: Record<string, unknown>) => void>;
const start = () => startFrameLoadProbe(label, () => null, 0, host, log);

beforeEach(() => {
  vi.useFakeTimers();
  log = vi.fn();
});
afterEach(() => vi.useRealTimers());

describe('a frame that finishes in time', () => {
  it('writes no line', () => {
    const probe = start();
    vi.advanceTimersByTime(400);
    probe.noteLoad();
    probe.noteRevealed(false);
    vi.advanceTimersByTime(SLOW_LOAD_MS * 2);
    probe.abandon();
    expect(log).not.toHaveBeenCalled();
  });

  it('writes no line when it leaves before the limit', () => {
    const probe = start();
    vi.advanceTimersByTime(SLOW_LOAD_MS - 1);
    probe.abandon();
    vi.advanceTimersByTime(SLOW_LOAD_MS);
    expect(log).not.toHaveBeenCalled();
  });
});

describe('a frame still covered after the limit', () => {
  it('writes one slow load line with every field', () => {
    const probe = start();
    vi.advanceTimersByTime(300);
    probe.noteLoad();
    vi.advanceTimersByTime(SLOW_LOAD_MS);

    expect(log).toHaveBeenCalledTimes(1);
    const [category, message, data] = log.mock.calls[0];
    expect(category).toBe('app-frame');
    expect(message).toBe('slow load');
    expect(data).toEqual({
      app: 'thread-cost',
      place: 'dashboard',
      sinceMountMs: SLOW_LOAD_MS,
      loaded: true,
      ready: false,
      loadMs: 300,
      readyMs: null,
      revealMs: null,
      fused: false,
      ...host(),
      frameDocument: null,
    });
  });

  it('reports an unloaded frame as not loaded', () => {
    start();
    vi.advanceTimersByTime(SLOW_LOAD_MS);
    expect(log.mock.calls[0][2]).toMatchObject({ loaded: false, ready: false, loadMs: null });
  });

  it('writes one ended line when the cover lifts after the load', () => {
    const probe = start();
    probe.noteLoad();
    vi.advanceTimersByTime(SLOW_LOAD_MS);
    vi.advanceTimersByTime(7000);
    probe.noteReady();
    probe.noteRevealed(false);
    probe.noteRevealed(false);
    probe.abandon();

    expect(log.mock.calls.map((c) => c[1])).toEqual(['slow load', 'slow load ended']);
    expect(log.mock.calls[1][2]).toMatchObject({
      sinceMountMs: SLOW_LOAD_MS + 7000,
      loadMs: 0,
      readyMs: SLOW_LOAD_MS + 7000,
      revealMs: SLOW_LOAD_MS + 7000,
      fused: false,
    });
  });

  it('waits for the load too, and names a fuse that lifted the cover', () => {
    const probe = start();
    vi.advanceTimersByTime(SLOW_LOAD_MS);
    probe.noteRevealed(true);
    expect(log).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(5000);
    probe.noteLoad();
    expect(log.mock.calls[1][1]).toBe('slow load ended');
    expect(log.mock.calls[1][2]).toMatchObject({ fused: true, loadMs: SLOW_LOAD_MS + 5000 });
  });

  it('writes one abandoned line when the element goes first, and nothing after', () => {
    const probe = start();
    vi.advanceTimersByTime(SLOW_LOAD_MS + 500);
    probe.abandon();
    probe.noteLoad();
    probe.noteRevealed(false);
    probe.abandon();
    expect(log.mock.calls.map((c) => c[1])).toEqual(['slow load', 'slow load abandoned']);
  });
});

describe('the frame document timing on a line', () => {
  const frame = document.createElement('iframe');
  frame.setAttribute('src', '/app/x/');
  const entry = { startTime: 40, fetchStart: 41, requestStart: 50, responseStart: 80, responseEnd: 82 };
  const timing = { start: 40, fetchStart: 41, requestStart: 50, responseStart: 80, responseEnd: 82 };

  beforeEach(() => {
    vi.spyOn(performance, 'getEntriesByName').mockReturnValue([entry] as unknown as PerformanceEntry[]);
  });
  afterEach(() => vi.restoreAllMocks());

  it('is null before this element loads, since a lone load may be a sibling\'s', () => {
    const probe = startFrameLoadProbe(label, () => frame, 0, host, log);
    vi.advanceTimersByTime(SLOW_LOAD_MS);
    expect(log.mock.calls[0][2]).toMatchObject({ loaded: false, frameDocument: null });
    probe.abandon();
    expect(log.mock.calls[1][2]).toMatchObject({ frameDocument: null });
  });

  it('is read once the element loaded, on the slow line and the ended line', () => {
    const probe = startFrameLoadProbe(label, () => frame, 0, host, log);
    probe.noteLoad();
    vi.advanceTimersByTime(SLOW_LOAD_MS);
    probe.noteRevealed(false);
    expect(log.mock.calls.map((c) => c[2].frameDocument)).toEqual([timing, timing]);
  });

  it('reaches the ended line of a frame whose load came after the limit', () => {
    const probe = startFrameLoadProbe(label, () => frame, 0, host, log);
    vi.advanceTimersByTime(SLOW_LOAD_MS);
    probe.noteRevealed(true);
    probe.noteLoad();
    expect(log.mock.calls.map((c) => c[2].frameDocument)).toEqual([null, timing]);
  });
});

describe('frameDocumentTiming', () => {
  const ATTACHED_AT = 5000;
  const load = (startTime: number, requestStart = startTime + 10.2) => ({
    startTime,
    fetchStart: startTime + 0.8,
    requestStart,
    responseStart: startTime + 39.6,
    responseEnd: startTime + 41.1,
  });

  function frameAt(src: string): HTMLIFrameElement {
    const frame = document.createElement('iframe');
    frame.setAttribute('src', src);
    return frame;
  }

  const entries = (...loads: ReturnType<typeof load>[]) =>
    vi.spyOn(performance, 'getEntriesByName').mockReturnValue(loads as unknown as PerformanceEntry[]);

  afterEach(() => vi.restoreAllMocks());

  it('reads the load this element started, in milliseconds from its attachment', () => {
    const byName = entries(load(ATTACHED_AT + 100.4));
    const frame = frameAt('/app/x/?a=1#top');
    expect(frameDocumentTiming(frame, ATTACHED_AT)).toEqual({
      start: 100, fetchStart: 101, requestStart: 111, responseStart: 140, responseEnd: 142,
    });
    expect(byName).toHaveBeenCalledWith(frame.src.split('#')[0], 'resource');
  });

  it('is null when the only load of that url began before this element', () => {
    entries(load(3574));
    expect(frameDocumentTiming(frameAt('/app/x/'), ATTACHED_AT)).toBeNull();
  });

  it('skips the load of an earlier element and reads its own', () => {
    entries(load(3574), load(ATTACHED_AT + 20));
    expect(frameDocumentTiming(frameAt('/app/x/'), ATTACHED_AT)).toMatchObject({ start: 20 });
  });

  it('is null when two loads of that url began since, so neither is known to be its own', () => {
    entries(load(ATTACHED_AT + 5), load(ATTACHED_AT + 6));
    expect(frameDocumentTiming(frameAt('/app/x/'), ATTACHED_AT)).toBeNull();
  });

  it('reports a phase the browser hides as null, never as zero', () => {
    entries(load(ATTACHED_AT + 100, 0));
    expect(frameDocumentTiming(frameAt('/app/x/'), ATTACHED_AT)).toMatchObject({
      start: 100, requestStart: null,
    });
  });

  it('is null when the browser exposes none, or the frame has no url', () => {
    entries();
    expect(frameDocumentTiming(frameAt('/app/x/'), ATTACHED_AT)).toBeNull();
    expect(frameDocumentTiming(null, ATTACHED_AT)).toBeNull();
    expect(frameDocumentTiming(document.createElement('iframe'), ATTACHED_AT)).toBeNull();
  });

  it('is null when the performance timeline throws', () => {
    vi.spyOn(performance, 'getEntriesByName').mockImplementation(() => { throw new Error('unsupported'); });
    expect(frameDocumentTiming(frameAt('/app/x/'), ATTACHED_AT)).toBeNull();
  });
});
