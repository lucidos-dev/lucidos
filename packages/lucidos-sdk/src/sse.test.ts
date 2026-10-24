/**
 * How one frame reaches the listeners an app registered with `lucidos.sse.on`,
 * and how a stream (re)open reaches `lucidos.sse.onOpen`.
 *
 * Each transport is stubbed at its edge: a fake `EventSource` for the direct
 * one, a fake `SharedWorker` port for the shared one, and a fake host behind
 * `_bridge` for the relayed one. The real transport code runs in between.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const host = vi.hoisted(() => ({
  bridged: false,
  push: null as ((data: unknown) => void) | null,
  calls: [] as Array<{ op: string; args: unknown }>,
}));

vi.mock('./_bridge', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./_bridge')>()),
  isBridged: () => host.bridged,
  onHostPush: (_channel: string, handler: (data: unknown) => void) => {
    host.push = handler;
    return () => { host.push = null; };
  },
  callHost: (op: string, args: unknown) => {
    host.calls.push({ op, args });
    return Promise.resolve(null);
  },
  tellHost: (op: string, args: unknown) => {
    host.calls.push({ op, args });
  },
}));

import { EVENT_STREAM_RETRY_MS } from './eventStream';
import { sse } from './sse';

class FakeEventSource {
  static last: FakeEventSource | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public readonly url: string) {
    FakeEventSource.last = this;
  }
  addEventListener(): void {}
  close(): void {}
}

function deliver(data: string): void {
  FakeEventSource.last!.onmessage!({ data });
}

describe('lucidos.sse dispatch', () => {
  beforeEach(() => {
    vi.stubGlobal('EventSource', FakeEventSource);
    vi.stubGlobal('SharedWorker', undefined);
    vi.useFakeTimers();
    sse.connect();
  });

  afterEach(() => {
    sse.disconnect();
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  // A listener that throws must not stop later listeners, and its error must
  // still reach the console.
  it('a listener that throws costs no other listener its frame', () => {
    const after = vi.fn();
    const wildcard = vi.fn();
    const watchdogOnly = vi.getTimerCount();
    const offs = [
      sse.on('PreferencesChanged', () => { throw new Error('app bug'); }),
      sse.on('PreferencesChanged', after),
      sse.on('*', wildcard),
    ];
    deliver(JSON.stringify({ type: 'PreferencesChanged', data: { key: 'theme' } }));
    expect(after).toHaveBeenCalledWith({ key: 'theme' }, expect.anything());
    expect(wildcard).toHaveBeenCalledOnce();
    // Rethrown on its own task, so the console still reports it.
    expect(vi.getTimerCount()).toBe(watchdogOnly + 1);
    for (const off of offs) off();
  });

  it('ignores a frame that is not an event', () => {
    const wildcard = vi.fn();
    const off = sse.on('*', wildcard);
    deliver('not json');
    deliver('null');
    deliver(JSON.stringify({ data: {} }));
    expect(wildcard).not.toHaveBeenCalled();
    off();
  });
});

/**
 * One transport per case set. Each exposes the three things its upstream can
 * do to the SDK: open, drop, and deliver a frame. A reopen is an open after a
 * drop.
 */
interface Transport {
  name: string;
  install(): void;
  open(): void;
  drop(): void;
  frame(data: string): void;
}

const direct: Transport = {
  name: 'direct EventSource',
  install() {
    FakeEventSource.last = null;
    vi.stubGlobal('EventSource', FakeEventSource);
    vi.stubGlobal('SharedWorker', undefined);
  },
  // After a drop the SDK rebuilds the EventSource once the retry delay has
  // passed, and the open comes from that new one.
  open: () => {
    vi.advanceTimersByTime(EVENT_STREAM_RETRY_MS);
    FakeEventSource.last!.onopen!();
  },
  drop: () => FakeEventSource.last!.onerror!(),
  frame: (data) => FakeEventSource.last!.onmessage!({ data }),
};

interface FakePort {
  onmessage: ((e: { data: unknown }) => void) | null;
  postMessage(): void;
  start(): void;
  close(): void;
}
let port: FakePort;
const shared: Transport = {
  name: 'shared worker',
  install() {
    port = { onmessage: null, postMessage() {}, start() {}, close() {} };
    vi.stubGlobal('EventSource', FakeEventSource);
    vi.stubGlobal('SharedWorker', class { port = port; });
  },
  // The worker also sends this to a port that joins a stream already up.
  open: () => port.onmessage!({ data: { t: 'open' } }),
  drop: () => port.onmessage!({ data: { t: 'error' } }),
  frame: (data) => port.onmessage!({ data: { t: 'frame', data } }),
};

const bridged: Transport = {
  name: 'bridged host relay',
  install() {
    host.bridged = true;
  },
  open: () => host.push!({ kind: 'open' }),
  drop: () => host.push!({ kind: 'error' }),
  frame: (data) => host.push!({ kind: 'frame', data }),
};

describe.each([direct, shared, bridged])('lucidos.sse.onOpen over the $name', (transport) => {
  beforeEach(() => {
    transport.install();
    vi.useFakeTimers();
  });

  afterEach(() => {
    sse.disconnect();
    host.bridged = false;
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('runs on the first open, and connects the stream itself', () => {
    const opened = vi.fn();
    const off = sse.onOpen(opened);
    expect(opened).not.toHaveBeenCalled();
    transport.open();
    expect(opened).toHaveBeenCalledOnce();
    off();
  });

  it('runs again when the stream reopens after a drop', () => {
    const opened = vi.fn();
    const off = sse.onOpen(opened);
    transport.open();
    transport.drop();
    expect(opened).toHaveBeenCalledOnce();
    transport.open();
    expect(opened).toHaveBeenCalledTimes(2);
    off();
  });

  it('stops running once unsubscribed', () => {
    const opened = vi.fn();
    const off = sse.onOpen(opened);
    transport.open();
    off();
    transport.drop();
    transport.open();
    expect(opened).toHaveBeenCalledOnce();
  });

  it('runs once, at once, when it registers after the stream opened', async () => {
    // The SDK's own theme listener can open the stream before app code runs.
    // An app that missed the first open must still get its first render.
    const early = sse.onOpen(() => {});
    transport.open();
    const late = vi.fn();
    const off = sse.onOpen(late);
    await Promise.resolve();
    expect(late).toHaveBeenCalledOnce();
    early();
    off();
  });

  it('does not run at once when it registers while the stream is down', async () => {
    // Running then would read data over a dead stream, and run twice on the
    // real open that follows.
    const early = sse.onOpen(() => {});
    transport.open();
    transport.drop();
    const late = vi.fn();
    const off = sse.onOpen(late);
    await Promise.resolve();
    expect(late).not.toHaveBeenCalled();
    transport.open();
    expect(late).toHaveBeenCalledOnce();
    early();
    off();
  });

  it('never runs for an event frame, whatever its type', () => {
    const opened = vi.fn();
    const off = sse.onOpen(opened);
    transport.frame(JSON.stringify({ type: 'open', data: {} }));
    transport.frame(JSON.stringify({ type: 'Open', data: {} }));
    expect(opened).not.toHaveBeenCalled();
    off();
  });
});

describe('lucidos.sse over a direct EventSource that gave up', () => {
  // A direct EventSource stops retrying for good on a response that is not a
  // stream, such as a 503 page while the engine restarts. The SDK rebuilds it,
  // or `onOpen` would never run again in that tab.
  beforeEach(() => {
    direct.install();
    vi.useFakeTimers();
  });

  afterEach(() => {
    sse.disconnect();
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('builds a new EventSource after the retry delay, and its open runs onOpen', () => {
    const opened = vi.fn();
    const off = sse.onOpen(opened);
    const first = FakeEventSource.last!;
    first.onopen!();
    first.onerror!();

    vi.advanceTimersByTime(EVENT_STREAM_RETRY_MS - 1);
    expect(FakeEventSource.last).toBe(first);
    vi.advanceTimersByTime(1);
    expect(FakeEventSource.last).not.toBe(first);

    FakeEventSource.last!.onopen!();
    expect(opened).toHaveBeenCalledTimes(2);
    off();
  });

  it('builds nothing once disconnected during the wait', () => {
    const off = sse.onOpen(() => {});
    const first = FakeEventSource.last!;
    first.onerror!();
    sse.disconnect();
    vi.advanceTimersByTime(EVENT_STREAM_RETRY_MS);
    expect(FakeEventSource.last).toBe(first);
    off();
  });
});

describe('lucidos.sse.onOpen isolation', () => {
  beforeEach(() => {
    direct.install();
    vi.useFakeTimers();
  });

  afterEach(() => {
    sse.disconnect();
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('a callback that throws costs no other callback its call', () => {
    const after = vi.fn();
    const offs = [sse.onOpen(() => { throw new Error('app bug'); }), sse.onOpen(after)];
    const watchdogOnly = vi.getTimerCount();
    direct.open();
    expect(after).toHaveBeenCalledOnce();
    // Rethrown on its own task, so the console still reports it.
    expect(vi.getTimerCount()).toBe(watchdogOnly + 1);
    for (const off of offs) off();
  });

  it('a late callback that a real open overtook runs only once', async () => {
    const early = sse.onOpen(() => {});
    direct.open();
    const late = vi.fn();
    const off = sse.onOpen(late);
    direct.drop();
    direct.open(); // lands before the microtask the late registration queued
    await Promise.resolve();
    expect(late).toHaveBeenCalledOnce();
    early();
    off();
  });

  it('a late callback unsubscribed before its microtask never runs', async () => {
    const early = sse.onOpen(() => {});
    direct.open();
    const late = vi.fn();
    sse.onOpen(late)();
    await Promise.resolve();
    expect(late).not.toHaveBeenCalled();
    early();
  });
});

/** Invariant: a frame receives only the event types it listens to. The SDK
 *  names them on open and after each change, so the host can filter. */
describe('lucidos.sse tells a relaying host what it listens to', () => {
  beforeEach(() => {
    host.bridged = true;
    host.calls.length = 0;
  });

  afterEach(() => {
    sse.disconnect();
    host.bridged = false;
  });

  it('names its types on open, then once per change', async () => {
    const offPinned = sse.on('WidgetPinned', () => {});
    sse.connect();
    expect(host.calls).toEqual([{ op: 'sse.open', args: { types: ['WidgetPinned'] } }]);

    const offNote = sse.on('NotificationCreated', () => {});
    const offAgain = sse.on('NotificationCreated', () => {});
    await Promise.resolve();
    expect(host.calls[host.calls.length - 1]).toEqual({
      op: 'sse.types',
      args: { types: ['NotificationCreated', 'WidgetPinned'] },
    });
    const sent = host.calls.length;

    offAgain();
    await Promise.resolve();
    expect(host.calls.length).toBe(sent);

    offNote();
    offPinned();
    await Promise.resolve();
    expect(host.calls[host.calls.length - 1]).toEqual({ op: 'sse.types', args: { types: [] } });
  });
});
