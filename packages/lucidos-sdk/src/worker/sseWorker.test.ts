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
