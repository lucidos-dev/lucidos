import { ApiError } from '../api/client/_core';
import { isTerminalDisabled, type TerminalSize } from '../api/client/terminal';
import type { TerminalExitCause } from '../generated/thread-event-wire';
import { encodeClientFrame, parseServerFrame, type ClientFrame } from './frames';

export type ConnectionState =
  | { kind: 'connecting' }
  | { kind: 'attached' }
  | { kind: 'reconnecting'; attempt: number }
  | { kind: 'ended'; cause: TerminalExitCause; status: number | null }
  | { kind: 'gone' }
  | { kind: 'disabled' };

export interface TerminalSink {
  write(data: Uint8Array): void;
  reset(): void;
  size(size: TerminalSize): void;
  attachments(count: number): void;
  state(state: ConnectionState): void;
}

export interface SocketLike {
  binaryType: BinaryType;
  readonly readyState: number;
  onopen: ((ev: Event) => void) | null;
  onmessage: ((ev: MessageEvent) => void) | null;
  onclose: ((ev: CloseEvent) => void) | null;
  send(data: string | ArrayBufferLike | ArrayBufferView): void;
  close(): void;
}

export interface ConnectionDeps {
  mintTicket(): Promise<string>;
  socketUrl(ticket: string): string;
  openSocket(url: string): SocketLike;
  schedule(run: () => void, ms: number): () => void;
}

const OPEN = 1;
export const BACKOFF_MS = [250, 500, 1000, 2000, 4000, 8000] as const;

export function backoffDelay(attempt: number): number {
  return BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)];
}

// INVARIANT: a connection only reattaches. It never opens a shell: only the user's New shell does.
export class TerminalConnection {
  private socket: SocketLike | null = null;
  private offset: number | null = null;
  private attempt = 0;
  private cancelRetry: (() => void) | null = null;
  private finished = false;
  private size: TerminalSize;
  private firstTicket: string | null;

  constructor(
    private readonly deps: ConnectionDeps,
    private readonly sink: TerminalSink,
    size: TerminalSize,
    firstTicket: string | null,
  ) {
    this.size = size;
    this.firstTicket = firstTicket;
  }

  start(): void {
    this.sink.state({ kind: 'connecting' });
    void this.dial();
  }

  input(data: string | Uint8Array): void {
    if (this.socket?.readyState !== OPEN) return;
    this.socket.send(typeof data === 'string' ? new TextEncoder().encode(data) : data);
  }

  resize(size: TerminalSize): void {
    this.size = size;
    this.sendFrame({ type: 'resize', ...size });
  }

  focus(): void {
    this.sendFrame({ type: 'focus' });
  }

  resumeNow(): void {
    if (this.finished || !this.cancelRetry) return;
    this.cancelRetry();
    this.cancelRetry = null;
    void this.dial();
  }

  dispose(): void {
    this.finish();
  }

  private sendFrame(frame: ClientFrame): void {
    if (this.socket?.readyState === OPEN) this.socket.send(encodeClientFrame(frame));
  }

  private finish(state?: ConnectionState): void {
    this.finished = true;
    this.cancelRetry?.();
    this.cancelRetry = null;
    const socket = this.socket;
    this.socket = null;
    socket?.close();
    if (state) this.sink.state(state);
  }

  private async dial(): Promise<void> {
    let ticket: string;
    try {
      ticket = this.firstTicket ?? await this.deps.mintTicket();
    } catch (err) {
      this.onMintFailed(err);
      return;
    }
    this.firstTicket = null;
    if (this.finished) return;
    const socket = this.deps.openSocket(this.deps.socketUrl(ticket));
    socket.binaryType = 'arraybuffer';
    this.socket = socket;
    socket.onopen = () => {
      const hello: ClientFrame = { type: 'hello', ...this.size };
      if (this.offset !== null) hello.resume_from = this.offset;
      socket.send(encodeClientFrame(hello));
    };
    socket.onmessage = (ev) => this.onMessage(ev);
    socket.onclose = () => this.onClose(socket);
  }

  private onMintFailed(err: unknown): void {
    if (this.finished) return;
    if (isTerminalDisabled(err)) {
      this.finish({ kind: 'disabled' });
      return;
    }
    if (err instanceof ApiError && err.httpCode === 404) {
      this.finish({ kind: 'gone' });
      return;
    }
    this.retry();
  }

  private onMessage(ev: MessageEvent): void {
    if (typeof ev.data !== 'string') {
      const bytes = new Uint8Array(ev.data as ArrayBuffer);
      if (this.offset !== null) this.offset += bytes.length;
      this.sink.write(bytes);
      return;
    }
    const frame = parseServerFrame(ev.data);
    if (!frame) return;
    switch (frame.type) {
      case 'sync':
        if (frame.reset) this.sink.reset();
        this.offset = frame.offset;
        this.attempt = 0;
        this.sink.state({ kind: 'attached' });
        break;
      case 'size':
        this.sink.size({ cols: frame.cols, rows: frame.rows });
        break;
      case 'attachments':
        this.sink.attachments(frame.count);
        break;
      case 'exit':
        this.finish({ kind: 'ended', cause: frame.cause, status: frame.status });
        break;
    }
  }

  private onClose(socket: SocketLike): void {
    if (this.socket !== socket) return;
    this.socket = null;
    if (!this.finished) this.retry();
  }

  private retry(): void {
    const attempt = this.attempt;
    this.attempt += 1;
    this.sink.state({ kind: 'reconnecting', attempt: this.attempt });
    this.cancelRetry = this.deps.schedule(() => {
      this.cancelRetry = null;
      void this.dial();
    }, backoffDelay(attempt));
  }
}
