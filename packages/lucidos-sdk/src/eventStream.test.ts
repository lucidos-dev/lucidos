/**
 * @vitest-environment jsdom
 *
 * `postPong` is the single place a pong reaches the engine, shared by the
 * direct transport and the shared worker. The wire contract asserted here used
 * to live inside the host's own PresenceCheck handler, and moved down so the
 * two transports cannot drift on it.
 *
 * jsdom, because the shared transport listens for `pagehide` to announce a
 * departure the worker could not otherwise detect.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  LATE_JOIN_STALL_AFTER_MS,
  openEventStream,
  postPong,
  sharedWorkerAvailable,
  startLiveness,
  STREAM_STALL_AFTER_MS,
  type EventStreamTargets,
  type PongAnswer,
  type StreamDrop,
} from './eventStream';
import { SSE_KEEPALIVE_EVENT, SSE_KEEPALIVE_INTERVAL_MS } from './generated/engine-constants';

const TARGETS: EventStreamTargets = {
  streamUrl: 'http://test/api/v1/events',
  pongUrl: 'http://test/api/v1/presence-pong',
  workerUrl: 'http://test/api/v1/sse-worker.js',
};

const NO_OP_HANDLERS = { onFrame: () => {}, onOpen: () => {}, onError: () => {} };

const ANSWER: PongAnswer = { device_id: 'dev-test', outcome: 'shown' };

describe('postPong', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchMock = vi.fn(() => Promise.resolve(new Response(null, { status: 200 })));
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('POSTs the notification id merged into the answer', () => {
    postPong('http://test/api/v1/presence-pong', 'n-1', ANSWER);

    expect(fetchMock).toHaveBeenCalledOnce();
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('http://test/api/v1/presence-pong');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({
      notification_id: 'n-1',
      device_id: 'dev-test',
      outcome: 'shown',
    });
  });

  it('sets keepalive, so a pong survives the document going away', () => {
    // A notification can fire as the tab is closing. Without this the pong is
    // dropped mid-flight and the engine waits out its deadline for nothing.
    postPong('http://test/api/v1/presence-pong', 'n-1', ANSWER);
    expect(fetchMock.mock.calls[0][1].keepalive).toBe(true);
  });

  it('swallows a network failure instead of throwing at its caller', async () => {
    // Spec §3 failure handling: a missed pong is read as not-active, so the
    // user gets the OS push rather than nothing. SSE dispatch cannot recover
    // from a throw here, and the worker has no caller to surface one to.
    fetchMock.mockRejectedValue(new Error('network down'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});

    expect(() => postPong('http://test/api/v1/presence-pong', 'n-1', ANSWER)).not.toThrow();
    await new Promise((r) => setTimeout(r, 0));

    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});

/**
 * Transport selection. Every browser must end up with a working stream: the
 * shared one where it can, and today's direct one where it cannot. Falling
 * back to nothing is the failure these guard.
 */
describe('openEventStream', () => {
  /** Minimal stand-ins. The real ones need a browser, and what matters here is
   *  only WHICH constructor the picker reached for. */
  class FakeEventSource {
    static last: FakeEventSource | null = null;
    onmessage: ((e: MessageEvent) => void) | null = null;
    onopen: (() => void) | null = null;
    onerror: (() => void) | null = null;
    closed = false;
    private named = new Map<string, () => void>();
    constructor(public url: string) {
      FakeEventSource.last = this;
    }
    addEventListener(type: string, listener: () => void) {
      this.named.set(type, listener);
    }
    /** Deliver a named SSE event, as the browser does for `event: <type>`. */
    emit(type: string) {
      this.named.get(type)?.();
    }
    close() {
      this.closed = true;
    }
  }

  const fakePort = () => ({
    postMessage: vi.fn(),
    close: vi.fn(),
    start: vi.fn(),
    onmessage: null as unknown,
  });

  beforeEach(() => {
    FakeEventSource.last = null;
    vi.stubGlobal('EventSource', FakeEventSource);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('uses the shared worker when the browser has one', () => {
    const port = fakePort();
    vi.stubGlobal('EventSource', FakeEventSource);
    vi.stubGlobal('SharedWorker', class { port = port; });

    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });

    expect(FakeEventSource.last).toBeNull();
    expect(stream.ownsReconnect).toBe(true);
    expect(port.postMessage).toHaveBeenCalledWith({
      t: 'hello',
      pongs: true,
      streamUrl: TARGETS.streamUrl,
      pongUrl: TARGETS.pongUrl,
    });
  });

  it('falls back to a direct stream with no SharedWorker', () => {
    // Chromium on Android is the real case. It keeps exactly the behaviour it
    // had before the stream was ever shared, rather than losing events.
    vi.stubGlobal('SharedWorker', undefined);

    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });

    expect(sharedWorkerAvailable()).toBe(false);
    expect(FakeEventSource.last?.url).toBe(TARGETS.streamUrl);
    expect(stream.ownsReconnect).toBe(false);
  });

  it('falls back when the worker exists but cannot be constructed', () => {
    // A strict CSP or a private browsing mode can throw here. Failing to a
    // working stream beats failing to none.
    vi.stubGlobal('SharedWorker', class { constructor() { throw new Error('blocked'); } });
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});

    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });

    expect(FakeEventSource.last?.url).toBe(TARGETS.streamUrl);
    expect(stream.ownsReconnect).toBe(false);
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it('declares an app iframe as a non-ponger', () => {
    // An app holds a port and has no presence voice, exactly its position
    // today. The worker must not wait on it for a PresenceCheck answer.
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });

    openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: false });

    expect(port.postMessage.mock.calls[0][0].pongs).toBe(false);
  });

  it('relays a frame from the worker verbatim', () => {
    // The equivalence that everything downstream rests on: a relayed frame is
    // the same string a direct EventSource would have handed over.
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });
    const frames: string[] = [];

    openEventStream(TARGETS, { ...NO_OP_HANDLERS, onFrame: (d) => frames.push(d) }, { pongs: true });
    const onmessage = port.onmessage as unknown as (e: { data: unknown }) => void;
    onmessage({ data: { t: 'frame', data: '{"type":"NotificationCreated","data":{}}' } });

    expect(frames).toEqual(['{"type":"NotificationCreated","data":{}}']);
  });

  it('relays an open, a late-join open and a drop from the worker', () => {
    // `lucidos.sse.onOpen` rests on these: an app re-reads its data on each
    // open, the late-join one included, which a port joining a live stream gets.
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });
    const seen: string[] = [];

    openEventStream(TARGETS, {
      onFrame: () => {},
      onOpen: () => seen.push('open'),
      onError: () => seen.push('error'),
    }, { pongs: false });
    const onmessage = port.onmessage as unknown as (e: { data: unknown }) => void;
    onmessage({ data: { t: 'open', lateJoin: true } });
    onmessage({ data: { t: 'error' } });
    onmessage({ data: { t: 'open' } });

    expect(seen).toEqual(['open', 'error', 'open']);
  });

  it('relays an open and a drop from a direct stream', () => {
    vi.stubGlobal('SharedWorker', undefined);
    const seen: string[] = [];

    openEventStream(TARGETS, {
      onFrame: () => {},
      onOpen: () => seen.push('open'),
      onError: () => seen.push('error'),
    }, { pongs: false });
    FakeEventSource.last!.onopen!();
    FakeEventSource.last!.onerror!();
    FakeEventSource.last!.onopen!();

    expect(seen).toEqual(['open', 'error', 'open']);
  });

  it('says goodbye when the document goes away without disconnecting', () => {
    // The common departure: a closed tab, a navigation, an iframe removed from
    // the DOM. None of them call disconnect(), and posting to a dead port
    // throws nothing, so without this the worker keeps the client forever.
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });

    openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });
    dispatchEvent(Object.assign(new Event('pagehide'), { persisted: false }));

    expect(port.postMessage).toHaveBeenLastCalledWith({ t: 'bye' });
  });

  it('stays attached through a bfcache pagehide, which comes back', () => {
    // A persisted pagehide keeps this document and its port alive, and the page
    // resumes with this same JS state. Leaving would strand it on return.
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });

    openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });
    dispatchEvent(Object.assign(new Event('pagehide'), { persisted: true }));

    expect(port.postMessage).toHaveBeenCalledTimes(1); // the hello, and nothing else
  });

  it('stops listening for pagehide once closed', () => {
    // A closed transport that still answered pagehide would post through a port
    // it had already given up.
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });

    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });
    stream.close();
    const afterClose = port.postMessage.mock.calls.length;
    dispatchEvent(Object.assign(new Event('pagehide'), { persisted: false }));

    expect(port.postMessage.mock.calls.length).toBe(afterClose);
  });

  it('sends a pong through the worker rather than POSTing it directly', () => {
    // This is what holds the engine's expected_pong_count equal to its
    // open-connection count: the worker POSTs one pong for the workspace.
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });
    const fetchMock = vi.fn(() => Promise.resolve(new Response(null, { status: 200 })));
    vi.stubGlobal('fetch', fetchMock);

    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });
    stream.submitPong('n-1', ANSWER);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(port.postMessage).toHaveBeenLastCalledWith({
      t: 'pong',
      notificationId: 'n-1',
      answer: ANSWER,
    });
  });
});

describe('startLiveness', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('calls back once, with the silence length, when nothing arrives', () => {
    const onStall = vi.fn();
    startLiveness(onStall, 1000);

    vi.advanceTimersByTime(999);
    expect(onStall).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onStall).toHaveBeenCalledExactlyOnceWith(1000);
    vi.advanceTimersByTime(10_000);
    expect(onStall).toHaveBeenCalledOnce();
  });

  it('is pushed back by every touch', () => {
    const onStall = vi.fn();
    const liveness = startLiveness(onStall, 1000);

    for (let i = 0; i < 5; i++) {
      vi.advanceTimersByTime(900);
      liveness.touch();
    }
    expect(onStall).not.toHaveBeenCalled();
    expect(liveness.silentMs()).toBe(0);
  });

  it('stays quiet once stopped', () => {
    const onStall = vi.fn();
    startLiveness(onStall, 1000).stop();

    vi.advanceTimersByTime(5000);
    expect(onStall).not.toHaveBeenCalled();
  });
});

/**
 * The direct transport's watchdog. A stream that goes quiet without erroring,
 * a half-open connection after a phone sleeps, must still reach `onError`.
 */
describe('direct stream watchdog', () => {
  class WatchedEventSource {
    static last: WatchedEventSource | null = null;
    onmessage: ((e: MessageEvent) => void) | null = null;
    onopen: (() => void) | null = null;
    onerror: (() => void) | null = null;
    closed = false;
    private named = new Map<string, () => void>();
    constructor(public url: string) {
      WatchedEventSource.last = this;
    }
    addEventListener(type: string, listener: () => void) {
      this.named.set(type, listener);
    }
    emit(type: string) {
      this.named.get(type)?.();
    }
    close() {
      this.closed = true;
    }
  }

  const open = () => {
    const frames: string[] = [];
    const drops: StreamDrop[] = [];
    const stream = openEventStream(
      TARGETS,
      {
        onFrame: (d) => frames.push(d),
        onOpen: () => {},
        onError: (drop) => drops.push(drop),
      },
      { pongs: false },
    );
    return { frames, drops, stream, es: WatchedEventSource.last! };
  };

  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal('SharedWorker', undefined);
    vi.stubGlobal('EventSource', WatchedEventSource);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('closes and reports a stall once nothing has arrived for two intervals and slack', () => {
    const { drops, es } = open();

    vi.advanceTimersByTime(STREAM_STALL_AFTER_MS - 1);
    expect(drops).toEqual([]);
    vi.advanceTimersByTime(1);

    expect(es.closed).toBe(true);
    expect(drops).toEqual([
      { reason: 'stalled', silentMs: STREAM_STALL_AFTER_MS, transport: 'direct' },
    ]);
  });

  it('stays open indefinitely while keepalives arrive on time', () => {
    const { drops, es } = open();

    for (let i = 0; i < 20; i++) {
      vi.advanceTimersByTime(SSE_KEEPALIVE_INTERVAL_MS);
      es.emit(SSE_KEEPALIVE_EVENT);
    }

    expect(es.closed).toBe(false);
    expect(drops).toEqual([]);
  });

  it('counts a frame as life too', () => {
    const { drops, es } = open();

    vi.advanceTimersByTime(STREAM_STALL_AFTER_MS - 1);
    es.onmessage!({ data: '{"type":"X"}' } as MessageEvent);
    vi.advanceTimersByTime(STREAM_STALL_AFTER_MS - 1);

    expect(drops).toEqual([]);
  });

  it('never hands a keepalive to the frame consumer', () => {
    const { frames, es } = open();

    es.emit(SSE_KEEPALIVE_EVENT);

    expect(frames).toEqual([]);
  });

  it('reports an error with no stall reason', () => {
    const { drops, es } = open();

    es.onerror!();

    expect(drops).toEqual([{ reason: 'error' }]);
  });

  it('stops watching once closed', () => {
    const { drops, stream } = open();

    stream.close();
    vi.advanceTimersByTime(STREAM_STALL_AFTER_MS * 2);

    expect(drops).toEqual([]);
  });

  it('revalidates as live until one keepalive and slack of silence', () => {
    const { stream } = open();

    vi.advanceTimersByTime(LATE_JOIN_STALL_AFTER_MS - 1);

    expect(stream.revalidate()).toBe('live');
  });

  it('revalidates as stale once silent for the late-join threshold', () => {
    const { stream } = open();

    vi.advanceTimersByTime(LATE_JOIN_STALL_AFTER_MS);

    expect(stream.revalidate()).toBe('stale');
  });

  it('revalidates as live again after a keepalive', () => {
    const { stream, es } = open();

    vi.advanceTimersByTime(LATE_JOIN_STALL_AFTER_MS);
    es.emit(SSE_KEEPALIVE_EVENT);

    expect(stream.revalidate()).toBe('live');
  });

  it('revalidating never closes the stream', () => {
    const { stream, es } = open();

    stream.revalidate();

    expect(es.closed).toBe(false);
  });
});

describe('shared stream revalidate', () => {
  it('asks the worker to check its upstream and reports the check as pending', () => {
    const port = {
      postMessage: vi.fn(),
      close: vi.fn(),
      start: vi.fn(),
      onmessage: null as unknown,
    };
    vi.stubGlobal('SharedWorker', class { port = port; });

    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: false });

    expect(stream.revalidate()).toBe('checking');
    expect(port.postMessage).toHaveBeenLastCalledWith({ t: 'check' });
    vi.unstubAllGlobals();
  });
});

describe('shared stream drop', () => {
  it('relays a worker stall as a shared stall, and a bare error as an error', () => {
    const port = {
      postMessage: vi.fn(),
      close: vi.fn(),
      start: vi.fn(),
      onmessage: null as unknown,
    };
    vi.stubGlobal('SharedWorker', class { port = port; });
    const drops: StreamDrop[] = [];

    openEventStream(TARGETS, { ...NO_OP_HANDLERS, onError: (d) => drops.push(d) }, { pongs: false });
    const onmessage = port.onmessage as unknown as (e: { data: unknown }) => void;
    onmessage({ data: { t: 'error', silentMs: 65_000 } });
    onmessage({ data: { t: 'error' } });

    expect(drops).toEqual([
      { reason: 'stalled', silentMs: 65_000, transport: 'shared' },
      { reason: 'error' },
    ]);
    vi.unstubAllGlobals();
  });
});
