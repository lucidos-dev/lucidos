/**
 * The shared worker's upstream watchdog and its late-join restart.
 *
 * `sseWorker.ts` installs an `onconnect` handler at import and keeps its state
 * at module level. Each test imports a fresh copy against a fake `self` and a
 * fake `EventSource`, and drives it through fake ports.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LATE_JOIN_STALL_AFTER_MS, STREAM_STALL_AFTER_MS } from '../eventStream';
import { SSE_KEEPALIVE_EVENT, SSE_KEEPALIVE_INTERVAL_MS } from '../generated/engine-constants';

class FakeEventSource {
  static all: FakeEventSource[] = [];
  onmessage: ((e: MessageEvent) => void) | null = null;
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;
  private named = new Map<string, () => void>();
  constructor(public url: string) {
    FakeEventSource.all.push(this);
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
  static get live(): FakeEventSource[] {
    return FakeEventSource.all.filter((es) => !es.closed);
  }
}

interface FakePort {
  received: Array<Record<string, unknown>>;
  send(msg: unknown): void;
}

/** A port the worker can attach, recording what the worker posts to it. */
function fakePort(): FakePort & MessagePort {
  const port = {
    received: [] as Array<Record<string, unknown>>,
    onmessage: null as ((e: MessageEvent) => void) | null,
    postMessage(msg: Record<string, unknown>) {
      port.received.push(msg);
    },
    start() {},
    close() {},
    send(msg: unknown) {
      port.onmessage!({ data: msg } as MessageEvent);
    },
  };
  return port as unknown as FakePort & MessagePort;
}

const HELLO = {
  t: 'hello',
  pongs: false,
  streamUrl: 'http://test/api/v1/events',
  pongUrl: 'http://test/api/v1/presence-pong',
};

/** Load a fresh worker and return what the browser calls for each port, before
 *  the page has said anything. A relaunched worker gets exactly this for every
 *  port its previous instance held. */
async function loadWorker() {
  const scope: { onconnect?: (e: { ports: MessagePort[] }) => void } = {};
  vi.stubGlobal('self', scope);
  vi.resetModules();
  await import('./sseWorker');
  return (port: FakePort & MessagePort) => scope.onconnect!({ ports: [port] });
}

async function startWorker() {
  const connect = await loadWorker();
  return (port: FakePort & MessagePort) => {
    connect(port);
    port.send(HELLO);
  };
}

const INTRODUCE = { t: 'introduce' };

/** What the worker told a port after its greeting. */
const types = (port: FakePort) => port.received.slice(1).map((m) => m.t);
const lastReceived = (port: FakePort) => port.received[port.received.length - 1];

describe('shared worker upstream watchdog', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    FakeEventSource.all = [];
    vi.stubGlobal('EventSource', FakeEventSource);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('fences collections across restore and rejects duplicate or uninvited answers', async () => {
    const connect = await loadWorker();
    const ports = [fakePort(), fakePort(), fakePort()];
    for (const port of ports) { connect(port); port.send({ ...HELLO, pongs: true }); }
    const fetchMock = vi.fn().mockResolvedValue({});
    vi.stubGlobal('fetch', fetchMock);
    const es = FakeEventSource.live[0];
    const frame = (type: string, data: unknown) =>
      es.onmessage!({ data: JSON.stringify({ type, data }) } as MessageEvent);
    const pong = (sourceGeneration: string | null) => ({
      t: 'pong', notificationId: 'same-id', sourceGeneration,
      answer: { device_id: 'device', outcome: 'shown' },
    });
    frame('StreamHello', { restore_generation: null });
    frame('PresenceCheck', { notification_id: 'same-id' });
    ports[0].send(pong(null));
    frame('StreamHello', { restore_generation: 'restored' });
    frame('PresenceCheck', { notification_id: 'same-id' });
    ports[0].send(pong(null));
    ports[1].send(pong('restored'));
    ports[1].send(pong('restored'));
    const late = fakePort();
    connect(late); late.send({ ...HELLO, pongs: true });
    late.send(pong('restored'));
    expect(fetchMock).not.toHaveBeenCalled();
    vi.advanceTimersByTime(300);
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(new Headers(fetchMock.mock.calls[0][1].headers)
      .get('x-lucidos-restore-generation')).toBe('restored');
  });

  it('discards collections when their upstream closes', async () => {
    const connect = await loadWorker();
    const port = fakePort();
    connect(port); port.send({ ...HELLO, pongs: true });
    const fetchMock = vi.fn().mockResolvedValue({});
    vi.stubGlobal('fetch', fetchMock);
    const es = FakeEventSource.live[0];
    es.onmessage!({ data: JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } }) } as MessageEvent);
    es.onmessage!({ data: JSON.stringify({ type: 'PresenceCheck', data: { notification_id: 'old' } }) } as MessageEvent);
    port.send({ t: 'restart' });
    port.send({ t: 'pong', notificationId: 'old', sourceGeneration: null,
      answer: { device_id: 'device', outcome: 'shown' } });
    vi.advanceTimersByTime(300);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('gives a late tab the current generation before any subsequent domain frame', async () => {
    const attach = await startWorker();
    const first = fakePort();
    attach(first);
    const es = FakeEventSource.live[0];
    es.onopen!();
    const hello = JSON.stringify({ type: 'StreamHello', data: { restore_generation: 'generation-one' } });
    es.onmessage!({ data: hello } as MessageEvent);
    const late = fakePort();
    attach(late);
    const domain = JSON.stringify({ type: 'ThreadCreated', data: {} });
    es.onmessage!({ data: domain } as MessageEvent);
    expect(late.received).toEqual([
      INTRODUCE, { t: 'open', lateJoin: true },
      { t: 'frame', data: hello }, { t: 'frame', data: domain },
    ]);
    expect(first.received.filter((m) => m.data === hello)).toHaveLength(1);
  });

  it('does not give a joining tab the previous upstream generation after a restart', async () => {
    const attach = await startWorker();
    const first = fakePort();
    attach(first);
    const old = FakeEventSource.live[0];
    old.onopen!();
    const oldHello = JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } });
    old.onmessage!({ data: oldHello } as MessageEvent);
    first.send({ t: 'restart' });
    const current = FakeEventSource.live[0];
    current.onopen!();
    // Already queued callbacks from the closed EventSource cannot reinstall
    // its generation, mark the replacement down, or broadcast stale frames.
    const before = first.received.length;
    old.onmessage!({ data: oldHello } as MessageEvent);
    old.onopen!();
    old.onerror!();
    expect(first.received).toHaveLength(before);
    expect(current.closed).toBe(false);
    const late = fakePort();
    attach(late);
    expect(late.received.some((m) => m.data === oldHello)).toBe(false);
    const hello = JSON.stringify({ type: 'StreamHello', data: { restore_generation: 'restored' } });
    current.onmessage!({ data: hello } as MessageEvent);
    expect(lastReceived(late)).toEqual({ t: 'frame', data: hello });
  });

  it('does not mistake a domain payload mentioning StreamHello for connection metadata', async () => {
    const attach = await startWorker();
    const first = fakePort();
    attach(first);
    const es = FakeEventSource.live[0];
    es.onopen!();
    const hello = JSON.stringify({ type: 'StreamHello', data: { restore_generation: null } });
    es.onmessage!({ data: hello } as MessageEvent);
    es.onmessage!({ data: JSON.stringify({ type: 'MessageReceived', data: { text: 'StreamHello' } }) } as MessageEvent);
    es.onmessage!({ data: 'malformed StreamHello' } as MessageEvent);
    const late = fakePort();
    attach(late);
    expect(late.received).toEqual([INTRODUCE, { t: 'open', lateJoin: true }, { t: 'frame', data: hello }]);
  });

  it('reopens a silent upstream and tells every port it stalled', async () => {
    const attach = await startWorker();
    const first = fakePort();
    const second = fakePort();
    attach(first);
    attach(second);
    FakeEventSource.live[0].onopen!();

    vi.advanceTimersByTime(STREAM_STALL_AFTER_MS);

    expect(FakeEventSource.all).toHaveLength(2);
    expect(FakeEventSource.all[0].closed).toBe(true);
    expect(FakeEventSource.live).toHaveLength(1);
    for (const port of [first, second]) {
      expect(lastReceived(port)).toEqual({ t: 'error', silentMs: STREAM_STALL_AFTER_MS });
    }
    FakeEventSource.live[0].onopen!();
    expect(lastReceived(first)).toEqual({ t: 'open' });
  });

  it('never reopens an upstream whose keepalives arrive on time', async () => {
    const attach = await startWorker();
    const port = fakePort();
    attach(port);
    const es = FakeEventSource.live[0];
    es.onopen!();

    for (let i = 0; i < 20; i++) {
      vi.advanceTimersByTime(SSE_KEEPALIVE_INTERVAL_MS);
      es.emit(SSE_KEEPALIVE_EVENT);
    }

    expect(FakeEventSource.all).toHaveLength(1);
    expect(types(port)).toEqual(['open']);
  });

  it('relays no frame for a keepalive', async () => {
    const attach = await startWorker();
    const port = fakePort();
    attach(port);

    FakeEventSource.live[0].emit(SSE_KEEPALIVE_EVENT);

    expect(types(port)).toEqual([]);
  });

  it('restarts a silent upstream when a late port attaches', async () => {
    const attach = await startWorker();
    const first = fakePort();
    attach(first);
    FakeEventSource.live[0].onopen!();

    vi.advanceTimersByTime(LATE_JOIN_STALL_AFTER_MS);
    const late = fakePort();
    attach(late);

    expect(FakeEventSource.all).toHaveLength(2);
    expect(FakeEventSource.all[0].closed).toBe(true);
    expect(lastReceived(first)).toMatchObject({ t: 'error' });
    expect(lastReceived(late)).toMatchObject({ t: 'error' });
    expect(types(late)).not.toContain('open');
  });

  it('keeps a live upstream when a late port attaches', async () => {
    const attach = await startWorker();
    const first = fakePort();
    attach(first);
    const es = FakeEventSource.live[0];
    es.onopen!();

    vi.advanceTimersByTime(LATE_JOIN_STALL_AFTER_MS - 1);
    const late = fakePort();
    attach(late);

    expect(FakeEventSource.all).toHaveLength(1);
    expect(es.closed).toBe(false);
    expect(types(first)).toEqual(['open']);
    expect(late.received).toEqual([INTRODUCE, { t: 'open', lateJoin: true }]);
  });

  it('restarts a silent upstream when a port asks for a check', async () => {
    const attach = await startWorker();
    const port = fakePort();
    attach(port);
    FakeEventSource.live[0].onopen!();

    vi.advanceTimersByTime(LATE_JOIN_STALL_AFTER_MS);
    port.send({ t: 'check' });

    expect(FakeEventSource.all).toHaveLength(2);
    expect(FakeEventSource.all[0].closed).toBe(true);
    expect(port.received.slice(-2)).toEqual([
      { t: 'error', silentMs: LATE_JOIN_STALL_AFTER_MS },
      { t: 'checked' },
    ]);
    FakeEventSource.live[0].onopen!();
    expect(lastReceived(port)).toEqual({ t: 'open' });
  });

  it('keeps a live upstream when a port asks for a check, and answers it', async () => {
    const attach = await startWorker();
    const port = fakePort();
    attach(port);
    const es = FakeEventSource.live[0];
    es.onopen!();

    vi.advanceTimersByTime(LATE_JOIN_STALL_AFTER_MS - 1);
    port.send({ t: 'check' });

    expect(FakeEventSource.all).toHaveLength(1);
    expect(es.closed).toBe(false);
    expect(types(port)).toEqual(['open', 'checked']);
  });

  it('greets each port as it attaches, before the page says anything', async () => {
    const connect = await loadWorker();
    const port = fakePort();

    connect(port);

    expect(port.received).toEqual([INTRODUCE]);
    expect(FakeEventSource.all).toHaveLength(0);
  });

  it('opens no stream for a check from a port that never said hello, says it is down, and asks again', async () => {
    // A relaunched worker inherits its predecessor's ports with none of their
    // hellos. A check from one used to open `new EventSource('')`, which
    // fetched the worker's own script and failed with nobody told.
    const connect = await loadWorker();
    const orphan = fakePort();
    connect(orphan);

    orphan.send({ t: 'check' });

    expect(FakeEventSource.all).toHaveLength(0);
    expect(orphan.received).toEqual([INTRODUCE, { t: 'error' }, INTRODUCE, { t: 'checked' }]);
  });

  it('opens no stream for a restart from a port that never said hello, and asks again', async () => {
    const connect = await loadWorker();
    const orphan = fakePort();
    connect(orphan);

    orphan.send({ t: 'restart' });

    expect(FakeEventSource.all).toHaveLength(0);
    expect(orphan.received).toEqual([INTRODUCE, { t: 'error' }, INTRODUCE]);
  });

  it('serves an inherited port once its page says hello again', async () => {
    const connect = await loadWorker();
    const port = fakePort();
    connect(port);
    port.send({ t: 'check' });

    port.send(HELLO);
    FakeEventSource.live[0].onopen!();

    expect(FakeEventSource.all.map((es) => es.url)).toEqual([HELLO.streamUrl]);
    expect(lastReceived(port)).toEqual({ t: 'open' });
  });

  it('reopens a live upstream once for every port when a port asks to restart', async () => {
    const attach = await startWorker();
    const first = fakePort();
    const second = fakePort();
    attach(first);
    attach(second);
    FakeEventSource.live[0].onopen!();

    first.send({ t: 'restart' });

    expect(FakeEventSource.all).toHaveLength(2);
    expect(FakeEventSource.all[0].closed).toBe(true);
    expect(FakeEventSource.live).toHaveLength(1);
    for (const port of [first, second]) {
      expect(lastReceived(port)).toEqual({ t: 'error' });
    }
    FakeEventSource.live[0].onopen!();
    for (const port of [first, second]) {
      expect(lastReceived(port)).toEqual({ t: 'open' });
    }
  });

  it('retries a plain upstream error after the retry delay, with no stall reason', async () => {
    const attach = await startWorker();
    const port = fakePort();
    attach(port);

    FakeEventSource.live[0].onerror!();

    expect(lastReceived(port)).toEqual({ t: 'error' });
    expect(FakeEventSource.all).toHaveLength(1);
    vi.advanceTimersByTime(3000);
    expect(FakeEventSource.all).toHaveLength(2);
  });
});
