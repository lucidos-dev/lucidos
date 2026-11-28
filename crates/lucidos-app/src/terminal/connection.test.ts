import { describe, it, expect, vi } from 'vitest';
import { TERMINAL_DISABLED_REASON } from '@lucidos/engine-constants';
import { ApiError } from '../api/client/_core';
import { BACKOFF_MS, TerminalConnection, backoffDelay, type ConnectionState, type SocketLike } from './connection';

class FakeSocket implements SocketLike {
  binaryType: BinaryType = 'blob';
  readyState = 0;
  onopen: ((ev: Event) => void) | null = null;
  onmessage: ((ev: MessageEvent) => void) | null = null;
  onclose: ((ev: CloseEvent) => void) | null = null;
  sent: (string | Uint8Array)[] = [];
  closed = false;
  constructor(readonly url: string) {}
  send(data: string | ArrayBufferLike | ArrayBufferView): void {
    this.sent.push(typeof data === 'string' ? data : new Uint8Array(data as ArrayBuffer));
  }
  close(): void { this.closed = true; }
  open(): void {
    this.readyState = 1;
    this.onopen?.(new Event('open'));
  }
  text(frame: unknown): void {
    this.onmessage?.({ data: JSON.stringify(frame) } as MessageEvent);
  }
  bytes(data: string): void {
    this.onmessage?.({ data: new TextEncoder().encode(data).buffer } as MessageEvent);
  }
  drop(): void {
    this.readyState = 3;
    this.onclose?.({} as CloseEvent);
  }
  hello(): Record<string, unknown> {
    return JSON.parse(this.sent[0] as string);
  }
}

function harness(opts: { firstTicket?: string | null; mint?: () => Promise<string> } = {}) {
  const sockets: FakeSocket[] = [];
  const timers: { run: () => void; ms: number }[] = [];
  const states: ConnectionState[] = [];
  const written: string[] = [];
  let resets = 0;
  const mintTicket = vi.fn(opts.mint ?? (async () => 'minted'));
  const connection = new TerminalConnection(
    {
      mintTicket,
      socketUrl: (ticket) => `ws://x/socket?ticket=${ticket}`,
      openSocket: (url) => {
        const socket = new FakeSocket(url);
        sockets.push(socket);
        return socket;
      },
      schedule: (run, ms) => {
        const timer = { run, ms };
        timers.push(timer);
        return () => { timers.splice(timers.indexOf(timer), 1); };
      },
    },
    {
      write: (data) => written.push(new TextDecoder().decode(data)),
      reset: () => { resets++; },
      size: () => {},
      attachments: () => {},
      state: (state) => states.push(state),
    },
    { cols: 80, rows: 24 },
    opts.firstTicket === undefined ? 'first' : opts.firstTicket,
  );
  return { connection, sockets, timers, states, written, mintTicket, resets: () => resets };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

const lastState = (h: { states: ConnectionState[] }) => h.states[h.states.length - 1];

describe('a terminal connection', () => {
  it('dials with the ticket it was given, then says hello with no offset', async () => {
    const h = harness();
    h.connection.start();
    await flush();
    expect(h.mintTicket).not.toHaveBeenCalled();
    expect(h.sockets[0].url).toBe('ws://x/socket?ticket=first');
    h.sockets[0].open();
    expect(h.sockets[0].hello()).toEqual({ type: 'hello', cols: 80, rows: 24 });
  });

  it('resumes from the last byte it holds after a drop, with a fresh ticket', async () => {
    const h = harness();
    h.connection.start();
    await flush();
    const first = h.sockets[0];
    first.open();
    first.text({ type: 'sync', terminal_id: 't', offset: 100, reset: false });
    first.bytes('hello');
    first.drop();
    expect(lastState(h)).toEqual({ kind: 'reconnecting', attempt: 1 });
    expect(h.timers[0].ms).toBe(BACKOFF_MS[0]);
    h.timers[0].run();
    await flush();
    expect(h.mintTicket).toHaveBeenCalledTimes(1);
    const second = h.sockets[1];
    expect(second.url).toBe('ws://x/socket?ticket=minted');
    second.open();
    expect(second.hello()).toEqual({ type: 'hello', resume_from: 105, cols: 80, rows: 24 });
    expect(h.written).toEqual(['hello']);
  });

  it('clears the screen on a reset sync and takes its offset', async () => {
    const h = harness();
    h.connection.start();
    await flush();
    h.sockets[0].open();
    h.sockets[0].text({ type: 'sync', terminal_id: 't', offset: 7, reset: true });
    expect(h.resets()).toBe(1);
    expect(lastState(h)).toEqual({ kind: 'attached' });
  });

  it('backs off longer on each failed attempt, up to a ceiling', () => {
    expect(backoffDelay(0)).toBe(BACKOFF_MS[0]);
    expect(backoffDelay(2)).toBeGreaterThan(backoffDelay(1));
    expect(backoffDelay(99)).toBe(BACKOFF_MS[BACKOFF_MS.length - 1]);
  });

  it('stops for good on an exit frame and never dials again', async () => {
    const h = harness();
    h.connection.start();
    await flush();
    h.sockets[0].open();
    h.sockets[0].text({ type: 'exit', cause: 'killed', status: null });
    h.sockets[0].drop();
    expect(lastState(h)).toEqual({ kind: 'ended', cause: 'killed', status: null });
    expect(h.timers).toHaveLength(0);
    expect(h.sockets).toHaveLength(1);
  });

  it('reports a terminal that ended while away as gone, and opens no new shell', async () => {
    const h = harness({ firstTicket: null, mint: async () => { throw new ApiError(404, 'That terminal has ended.'); } });
    h.connection.start();
    await flush();
    expect(lastState(h)).toEqual({ kind: 'gone' });
    expect(h.sockets).toHaveLength(0);
    expect(h.timers).toHaveLength(0);
  });

  it('reports the switch being off, never a retry', async () => {
    const refusal = new ApiError(403, 'The terminal is off.', { reason: TERMINAL_DISABLED_REASON });
    const h = harness({ firstTicket: null, mint: async () => { throw refusal; } });
    h.connection.start();
    await flush();
    expect(lastState(h)).toEqual({ kind: 'disabled' });
    expect(h.timers).toHaveLength(0);
  });

  it('retries a transient mint failure', async () => {
    const h = harness({ firstTicket: null, mint: async () => { throw new ApiError(502, 'Bad gateway'); } });
    h.connection.start();
    await flush();
    expect(lastState(h)).toEqual({ kind: 'reconnecting', attempt: 1 });
    expect(h.timers).toHaveLength(1);
  });

  it('sends input only while the socket is open', async () => {
    const h = harness();
    h.connection.start();
    await flush();
    h.connection.input('ls');
    expect(h.sockets[0].sent).toHaveLength(0);
    h.sockets[0].open();
    h.connection.input('ls');
    expect(new TextDecoder().decode(h.sockets[0].sent[1] as Uint8Array)).toBe('ls');
  });

  it('dials at once when the page resumes during a backoff', async () => {
    const h = harness();
    h.connection.start();
    await flush();
    h.sockets[0].open();
    h.sockets[0].drop();
    h.connection.resumeNow();
    await flush();
    expect(h.timers).toHaveLength(0);
    expect(h.sockets).toHaveLength(2);
  });

  it('closes its socket and stays quiet once disposed', async () => {
    const h = harness();
    h.connection.start();
    await flush();
    h.sockets[0].open();
    h.connection.dispose();
    expect(h.sockets[0].closed).toBe(true);
    h.sockets[0].drop();
    expect(h.timers).toHaveLength(0);
  });
});
