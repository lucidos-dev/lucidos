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
  WORKER_ANSWER_MS,
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
    postPong('http://test/api/v1/presence-pong', 'n-1', ANSWER, null);

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
    postPong('http://test/api/v1/presence-pong', 'n-1', ANSWER, null);
    expect(fetchMock.mock.calls[0][1].keepalive).toBe(true);
  });

  it('swallows a network failure instead of throwing at its caller', async () => {
    fetchMock.mockRejectedValue(new Error('network down'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});

    expect(() => postPong('http://test/api/v1/presence-pong', 'n-1', ANSWER, null)).not.toThrow();
    await new Promise((r) => setTimeout(r, 0));

    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});

describe('openEventStream', () => {
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

  it('requires a hello and never borrows a restored generation for direct pongs', () => {
    vi.stubGlobal('SharedWorker', undefined);
    const fetchMock = vi.fn().mockResolvedValue({});
    vi.stubGlobal('fetch', fetchMock);
    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });
    stream.submitPong('early', ANSWER);
    expect(fetchMock).not.toHaveBeenCalled();
    const hello = (generation: string | null) => FakeEventSource.last!.onmessage!({
      data: JSON.stringify({ type: 'StreamHello', data: { restore_generation: generation } }),
    } as MessageEvent);
    hello('original');
    stream.submitPong('current', ANSWER);
    expect(new Headers(fetchMock.mock.calls[0][1].headers)
      .get('x-lucidos-restore-generation')).toBe('original');
    hello('restored');
    stream.submitPong('delayed', ANSWER);
    expect(fetchMock).toHaveBeenCalledOnce();
    stream.close();
  });

  it('keeps a shared document origin after a restored hello', () => {
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });
    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });
    const hello = (generation: string | null) =>
      (port.onmessage as (event: { data: unknown }) => void)({
        data: { t: 'frame', data: JSON.stringify({ type: 'StreamHello', data: { restore_generation: generation } }) },
      });
    stream.submitPong('early', ANSWER);
    expect(port.postMessage.mock.calls.some(([msg]) => msg.t === 'pong')).toBe(false);
    hello(null);
    stream.submitPong('current', ANSWER);
    expect(port.postMessage).toHaveBeenLastCalledWith({
      t: 'pong', notificationId: 'current', answer: ANSWER, sourceGeneration: null,
    });
    hello('restored');
    const count = port.postMessage.mock.calls.length;
    stream.submitPong('delayed', ANSWER);
    expect(port.postMessage).toHaveBeenCalledTimes(count);
    stream.close();
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
    vi.stubGlobal('SharedWorker', undefined);

    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });

    expect(sharedWorkerAvailable()).toBe(false);
    expect(FakeEventSource.last?.url).toBe(TARGETS.streamUrl);
    expect(stream.ownsReconnect).toBe(false);
  });

  it('falls back when the worker exists but cannot be constructed', () => {
    vi.stubGlobal('SharedWorker', class { constructor() { throw new Error('blocked'); } });
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});

    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });

    expect(FakeEventSource.last?.url).toBe(TARGETS.streamUrl);
    expect(stream.ownsReconnect).toBe(false);
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it('declares an app iframe as a non-ponger', () => {
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });

    openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: false });

    expect(port.postMessage.mock.calls[0][0].pongs).toBe(false);
  });

  it('relays a frame from the worker verbatim', () => {
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });
    const frames: string[] = [];

    openEventStream(TARGETS, { ...NO_OP_HANDLERS, onFrame: (d) => frames.push(d) }, { pongs: true });
    const onmessage = port.onmessage as unknown as (e: { data: unknown }) => void;
    onmessage({ data: { t: 'frame', data: '{"type":"NotificationCreated","data":{}}' } });

    expect(frames).toEqual(['{"type":"NotificationCreated","data":{}}']);
  });

  it('relays an open, a late-join open and a drop from the worker', () => {
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
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });

    openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });
    dispatchEvent(Object.assign(new Event('pagehide'), { persisted: false }));

    expect(port.postMessage).toHaveBeenLastCalledWith({ t: 'bye' });
  });

  it('stays attached through a bfcache pagehide, which comes back', () => {
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });

    openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });
    dispatchEvent(Object.assign(new Event('pagehide'), { persisted: true }));

    expect(port.postMessage).toHaveBeenCalledTimes(1);
  });

  it('stops listening for pagehide once closed', () => {
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });

    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });
    stream.close();
    const afterClose = port.postMessage.mock.calls.length;
    dispatchEvent(Object.assign(new Event('pagehide'), { persisted: false }));

    expect(port.postMessage.mock.calls.length).toBe(afterClose);
  });

  it('sends a pong through the worker rather than POSTing it directly', () => {
    const port = fakePort();
    vi.stubGlobal('SharedWorker', class { port = port; });
    const fetchMock = vi.fn(() => Promise.resolve(new Response(null, { status: 200 })));
    vi.stubGlobal('fetch', fetchMock);

    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: true });
    (port.onmessage as (event: { data: unknown }) => void)({
      data: { t: 'frame', data: JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }) },
    });
    stream.submitPong('n-1', ANSWER);

    expect(fetchMock).not.toHaveBeenCalled();
    expect(port.postMessage).toHaveBeenLastCalledWith({
      t: 'pong',
      notificationId: 'n-1',
      answer: ANSWER,
      sourceGeneration: null,
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

describe('shared stream recovery', () => {
  interface RecordingPort {
    sent: unknown[];
    closed: boolean;
    onmessage: ((e: { data: unknown }) => void) | null;
    postMessage(msg: unknown): void;
    start(): void;
    close(): void;
  }

  let ports: RecordingPort[];
  let drops: StreamDrop[];

  const HELLO = { t: 'hello', pongs: true, streamUrl: TARGETS.streamUrl, pongUrl: TARGETS.pongUrl };

  beforeEach(() => {
    vi.useFakeTimers();
    ports = [];
    drops = [];
    vi.stubGlobal(
      'SharedWorker',
      class {
        port: RecordingPort = {
          sent: [],
          closed: false,
          onmessage: null,
          postMessage(msg) {
            this.sent.push(msg);
          },
          start() {},
          close() {
            this.closed = true;
          },
        };
        constructor() {
          ports.push(this.port);
        }
      },
    );
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  const open = () =>
    openEventStream(TARGETS, { ...NO_OP_HANDLERS, onError: (d) => drops.push(d) }, { pongs: true });
  const fromWorker = (port: RecordingPort, msg: unknown) => port.onmessage!({ data: msg });

  it('takes the first request to introduce itself as answered by the hello in flight', () => {
    open();

    fromWorker(ports[0], { t: 'introduce' });

    expect(ports[0].sent).toEqual([HELLO]);
    expect(drops).toEqual([]);
  });

  it('says hello again and reports the drop when the worker asks a second time', () => {
    open();
    fromWorker(ports[0], { t: 'introduce' });

    fromWorker(ports[0], { t: 'introduce' });

    expect(ports[0].sent).toEqual([HELLO, HELLO]);
    expect(drops).toEqual([{ reason: 'worker-restarted' }]);
  });

  it('says hello again when a relaunched worker asks on a link an older worker served', () => {
    open();
    fromWorker(ports[0], { t: 'open' });
    fromWorker(ports[0], { t: 'frame', data: '{}' });

    fromWorker(ports[0], { t: 'introduce' });

    expect(ports[0].sent).toEqual([HELLO, HELLO]);
    expect(drops).toEqual([{ reason: 'worker-restarted' }]);
  });

  it('keeps replacing a worker that stays silent after a reattach', () => {
    const stream = open();
    fromWorker(ports[0], { t: 'introduce' });
    stream.revalidate();
    vi.advanceTimersByTime(WORKER_ANSWER_MS);

    stream.revalidate();
    vi.advanceTimersByTime(WORKER_ANSWER_MS);

    expect(ports).toHaveLength(3);
    expect(drops).toEqual([{ reason: 'worker-unresponsive' }, { reason: 'worker-unresponsive' }]);
  });

  it('asks again, rather than reattach, when the page froze while it waited', () => {
    const stream = open();
    fromWorker(ports[0], { t: 'introduce' });
    stream.revalidate();

    vi.setSystemTime(Date.now() + 60_000);
    vi.advanceTimersByTime(WORKER_ANSWER_MS);

    expect(ports).toHaveLength(1);
    expect(ports[0].sent.slice(-2)).toEqual([{ t: 'check' }, { t: 'check' }]);
    expect(drops).toEqual([]);

    fromWorker(ports[0], { t: 'checked' });
    vi.advanceTimersByTime(WORKER_ANSWER_MS);
    expect(ports).toHaveLength(1);
  });

  it('attaches to a new worker when a check goes unanswered', () => {
    const stream = open();
    fromWorker(ports[0], { t: 'introduce' });

    stream.revalidate();
    vi.advanceTimersByTime(WORKER_ANSWER_MS);

    expect(drops).toEqual([{ reason: 'worker-unresponsive' }]);
    expect(ports).toHaveLength(2);
    expect(ports[0].sent.slice(-1)).toEqual([{ t: 'bye' }]);
    expect(ports[0].closed).toBe(true);
    expect(ports[1].sent).toEqual([HELLO]);

    fromWorker(ports[1], { t: 'frame', data: JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }) });
    stream.submitPong('n-1', ANSWER);
    stream.close();
    expect(ports[1].sent.slice(1)).toEqual([
      { t: 'pong', notificationId: 'n-1', answer: ANSWER, sourceGeneration: null },
      { t: 'bye' },
    ]);
  });

  it('attaches to a new worker when a restart goes unanswered', () => {
    const stream = open();
    fromWorker(ports[0], { t: 'introduce' });

    stream.restart();
    vi.advanceTimersByTime(WORKER_ANSWER_MS);

    expect(drops).toEqual([{ reason: 'worker-unresponsive' }]);
    expect(ports).toHaveLength(2);
    expect(ports[1].sent).toEqual([HELLO]);
  });

  it('keeps a worker that answers the restart with its drop', () => {
    const stream = open();
    fromWorker(ports[0], { t: 'introduce' });

    stream.restart();
    fromWorker(ports[0], { t: 'error' });
    vi.advanceTimersByTime(WORKER_ANSWER_MS);

    expect(ports).toHaveLength(1);
    expect(drops).toEqual([{ reason: 'error' }]);
  });

  it('keeps a worker that answers the check', () => {
    const stream = open();
    fromWorker(ports[0], { t: 'introduce' });

    stream.revalidate();
    fromWorker(ports[0], { t: 'checked' });
    vi.advanceTimersByTime(WORKER_ANSWER_MS);

    expect(ports).toHaveLength(1);
    expect(drops).toEqual([]);
  });

  it('expects no answer from a worker that never asked, which predates the answer', () => {
    const stream = open();

    stream.revalidate();
    vi.advanceTimersByTime(WORKER_ANSWER_MS);

    expect(ports).toHaveLength(1);
    expect(drops).toEqual([]);
  });

  it('never reattaches once closed', () => {
    const stream = open();
    fromWorker(ports[0], { t: 'introduce' });

    stream.revalidate();
    stream.close();
    vi.advanceTimersByTime(WORKER_ANSWER_MS);

    expect(ports).toHaveLength(1);
    expect(drops).toEqual([]);
  });
});

describe('direct stream restart', () => {
  class RestartableEventSource {
    static last: RestartableEventSource | null = null;
    onmessage: ((e: MessageEvent) => void) | null = null;
    onopen: (() => void) | null = null;
    onerror: (() => void) | null = null;
    closed = false;
    constructor(public url: string) {
      RestartableEventSource.last = this;
    }
    addEventListener() {}
    close() {
      this.closed = true;
    }
  }

  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal('SharedWorker', undefined);
    vi.stubGlobal('EventSource', RestartableEventSource);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('closes the source and reports one drop so the consumer reconnects', () => {
    const drops: StreamDrop[] = [];
    const stream = openEventStream(
      TARGETS,
      { ...NO_OP_HANDLERS, onError: (d) => drops.push(d) },
      { pongs: false },
    );

    stream.restart();

    expect(RestartableEventSource.last!.closed).toBe(true);
    expect(drops).toEqual([{ reason: 'error' }]);
  });

  it('stops the watchdog, so the restarted stream never also reports a stall', () => {
    const drops: StreamDrop[] = [];
    const stream = openEventStream(
      TARGETS,
      { ...NO_OP_HANDLERS, onError: (d) => drops.push(d) },
      { pongs: false },
    );

    stream.restart();
    vi.advanceTimersByTime(STREAM_STALL_AFTER_MS * 2);

    expect(drops).toEqual([{ reason: 'error' }]);
  });
});

describe('shared stream restart', () => {
  it('asks the worker to reopen its upstream', () => {
    const port = {
      postMessage: vi.fn(),
      close: vi.fn(),
      start: vi.fn(),
      onmessage: null as unknown,
    };
    vi.stubGlobal('SharedWorker', class { port = port; });

    const stream = openEventStream(TARGETS, NO_OP_HANDLERS, { pongs: false });
    stream.restart();

    expect(port.postMessage).toHaveBeenLastCalledWith({ t: 'restart' });
    vi.unstubAllGlobals();
  });
});
