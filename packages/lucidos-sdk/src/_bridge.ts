
export const BRIDGE_TYPE = 'lucidos:bridge';
export const BRIDGE_REPLY_TYPE = 'lucidos:bridge:reply';
export const BRIDGE_PUSH_TYPE = 'lucidos:bridge:push';

export type BridgeOp =
  | 'media.poster'
  | 'fetch'
  | 'sse.open'
  | 'sse.types'
  | 'sse.close'
  | 'storage.prime'
  | 'storage.set'
  | 'storage.remove'
  | 'storage.clear'
  | 'ui.focused-thread'
  | 'log.app-link'
  | 'log.app-fullscreen';

export const FOCUSED_THREAD_CHANNEL = 'focused-thread';

export interface BridgeRequest {
  type: typeof BRIDGE_TYPE;
  id: string;
  op: BridgeOp;
  args: unknown;
}

export interface BridgeReply {
  type: typeof BRIDGE_REPLY_TYPE;
  id: string;
  ok: boolean;
  value?: unknown;
  error?: string;
}

export interface WireResponse {
  status: number;
  statusText: string;
  headers: Record<string, string>;
  body: ArrayBuffer;
}

export interface WireRequest {
  path: string;
  method: string;
  headers: Record<string, string>;
  body: WireBody;
  timeoutMs: number;
}

export type WireBody =
  | { kind: 'none' }
  | { kind: 'text'; value: string }
  | { kind: 'binary'; value: ArrayBuffer | Blob }
  | { kind: 'form'; entries: Array<[string, string | File]> };

let bridged: boolean | null = null;

export function isBridged(): boolean {
  if (bridged !== null) return bridged;
  bridged = typeof window !== 'undefined'
    && window.parent !== window
    && (globalThis as { origin?: string }).origin === 'null';
  return bridged;
}

export function _setBridgedForTesting(value: boolean | null): void {
  bridged = value;
}

export function fromHost(event: MessageEvent): boolean {
  const origin = hostOrigin();
  return origin !== null && event.source === window.parent && event.origin === origin;
}

export function postToHost(message: object): boolean {
  const origin = hostOrigin();
  if (origin === null) return false;
  window.parent.postMessage(message, origin);
  return true;
}

function hostOrigin(): string | null {
  let origin: string;
  try {
    origin = location.origin;
  } catch {
    return '*';
  }
  if (origin === 'null') return null;
  return origin || '*';
}

let counter = 0;
const pending = new Map<string, { resolve(v: unknown): void; reject(e: unknown): void }>();
const pushHandlers = new Map<string, Set<(data: unknown) => void>>();
let listening = false;

function listen(): void {
  if (listening) return;
  listening = true;
  window.addEventListener('message', (event: MessageEvent) => {
    if (!fromHost(event)) return;
    const data = event.data as { type?: string } | null;
    if (!data || typeof data !== 'object') return;
    if (data.type === BRIDGE_REPLY_TYPE) {
      const reply = data as BridgeReply;
      const slot = pending.get(reply.id);
      if (!slot) return;
      pending.delete(reply.id);
      if (reply.ok) slot.resolve(reply.value);
      else slot.reject(new Error(reply.error || 'the host refused the call'));
      return;
    }
    if (data.type === BRIDGE_PUSH_TYPE) {
      const push = data as unknown as { channel: string; data: unknown };
      for (const handler of pushHandlers.get(push.channel) ?? []) handler(push.data);
    }
  });
}

export function callHost(op: BridgeOp, args: unknown, timeoutMs = 30000): Promise<unknown> {
  listen();
  const id = `${++counter}-${performance.now()}`;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new DOMException(`Request timed out after ${timeoutMs}ms`, 'TimeoutError'));
    }, timeoutMs);
    pending.set(id, {
      resolve: (v) => { clearTimeout(timer); resolve(v); },
      reject: (e) => { clearTimeout(timer); reject(e); },
    });
    const message: BridgeRequest = { type: BRIDGE_TYPE, id, op, args };
    if (postToHost(message)) return;
    pending.get(id)?.reject(new Error('this frame has no host to call'));
    pending.delete(id);
  });
}

export function tellHost(op: BridgeOp, args: unknown): void {
  const message: BridgeRequest = { type: BRIDGE_TYPE, id: '', op, args };
  postToHost(message);
}

export const HOST_REQUEST_TYPE = 'lucidos:bridge:host';
export const HOST_REPLY_TYPE = 'lucidos:bridge:host:reply';

const hostHandlers = new Map<string, (args: unknown) => unknown>();
let serving = false;

export function serveHost(op: string, handler: (args: unknown) => unknown): void {
  hostHandlers.set(op, handler);
  if (serving) return;
  serving = true;
  window.addEventListener('message', (event: MessageEvent) => {
    if (!fromHost(event)) return;
    const data = event.data as { type?: string; id?: string; op?: string; args?: unknown } | null;
    if (!data || typeof data !== 'object' || data.type !== HOST_REQUEST_TYPE) return;
    const run = hostHandlers.get(String(data.op));
    const reply = (ok: boolean, payload: unknown) => {
      if (!data.id) return;
      postToHost({
        type: HOST_REPLY_TYPE,
        id: data.id,
        ok,
        [ok ? 'value' : 'error']: payload,
      });
    };
    if (!run) return reply(false, `this app has no "${data.op}" handler`);
    void Promise.resolve()
      .then(() => run(data.args))
      .then((value) => reply(true, value), (e: unknown) => reply(false, String(e)));
  });
}

export function _resetHostServingForTesting(): void {
  hostHandlers.clear();
  serving = false;
}

export function onHostPush(channel: string, handler: (data: unknown) => void): () => void {
  listen();
  let set = pushHandlers.get(channel);
  if (!set) {
    set = new Set();
    pushHandlers.set(channel, set);
  }
  set.add(handler);
  return () => { set?.delete(handler); };
}

export function headersToRecord(init: HeadersInit | undefined): Record<string, string> {
  if (!init) return {};
  const out: Record<string, string> = {};
  new Headers(init).forEach((value, name) => { out[name] = value; });
  return out;
}

export function toWireBody(body: BodyInit | null | undefined): WireBody {
  if (body === null || body === undefined) return { kind: 'none' };
  if (typeof body === 'string') return { kind: 'text', value: body };
  if (typeof FormData !== 'undefined' && body instanceof FormData) {
    return { kind: 'form', entries: [...body.entries()] as Array<[string, string | File]> };
  }
  if (body instanceof Blob || body instanceof ArrayBuffer) {
    return { kind: 'binary', value: body };
  }
  if (typeof URLSearchParams !== 'undefined' && body instanceof URLSearchParams) {
    return { kind: 'text', value: body.toString() };
  }
  if (ArrayBuffer.isView(body)) {
    return {
      kind: 'binary',
      value: body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength) as ArrayBuffer,
    };
  }
  throw new TypeError('This body shape cannot cross the app bridge');
}

const NULL_BODY_STATUS = new Set([101, 103, 204, 205, 304]);

export function fromWireResponse(wire: WireResponse): Response {
  return new Response(NULL_BODY_STATUS.has(wire.status) ? null : wire.body, {
    status: wire.status,
    statusText: wire.statusText,
    headers: wire.headers,
  });
}
