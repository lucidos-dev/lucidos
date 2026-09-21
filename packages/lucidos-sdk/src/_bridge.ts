/**
 * The app frame's transport to the host, for what an isolated frame cannot do
 * for itself.
 *
 * An app iframe runs in its own renderer process, so a busy app cannot freeze
 * the Lucidos shell. The price of that process is an opaque origin: `fetch` to
 * the engine is CORS-blocked at `Origin: null`, `EventSource` with it, and
 * `localStorage` throws. Those three go over `postMessage` to the host, which
 * makes the real call and hands the result back.
 *
 * Details, and the host half
 * (`crates/lucidos-app/src/store/actions/app-bridge.ts`), in
 * `docs/plans/2026-09-19-an-app-frame-cannot-starve-the-shell.md`.
 */

/** Every message in either direction carries this, so the host's other frame
 *  protocols (the confirm bridge, the shortcut forwarder) never collide. */
export const BRIDGE_TYPE = 'lucidos:bridge';
export const BRIDGE_REPLY_TYPE = 'lucidos:bridge:reply';
/** Host to app, unsolicited: an event-stream frame. */
export const BRIDGE_PUSH_TYPE = 'lucidos:bridge:push';

/** What the app asks the host to do on its behalf. */
export type BridgeOp =
  | 'fetch'
  | 'sse.open'
  | 'sse.close'
  | 'storage.prime'
  | 'storage.set'
  | 'storage.remove';

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

/** A `Response` flattened into something structured clone can carry. */
export interface WireResponse {
  status: number;
  statusText: string;
  headers: Record<string, string>;
  body: ArrayBuffer;
}

/** A `RequestInit` flattened the same way. `FormData` is not cloneable, so it
 *  travels as its entries; a `File` inside one is a `Blob` and is. */
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

/**
 * Is this frame isolated from the host, so the direct paths are closed?
 *
 * `window.origin === 'null'` is the test, and the two obvious alternatives are
 * both wrong. `location.origin` reports the document's URL origin even inside
 * the sandbox, so it never says null. A parent check alone is true for any
 * embedded frame, a same-origin one included.
 *
 * A standalone app tab answers false and keeps every direct path. It is a
 * top-level document on the engine's own origin.
 */
export function isBridged(): boolean {
  if (bridged !== null) return bridged;
  bridged = typeof window !== 'undefined'
    && window.parent !== window
    && (globalThis as { origin?: string }).origin === 'null';
  return bridged;
}

/** Test seam: force the answer, or `null` to derive it again. */
export function _setBridgedForTesting(value: boolean | null): void {
  bridged = value;
}

/** Where a reply may come from. Only the window we posted to may answer, the
 *  same rule the confirm bridge in `ui.ts` follows. A nested frame an app embeds
 *  can post to the app, and must not be able to resolve our calls. */
function fromHost(event: MessageEvent): boolean {
  return event.source === window.parent;
}

/** The host's origin, so a message cannot be aimed at some other document.
 *
 *  The sandbox leaves `location.origin` reporting the real one, which is the
 *  origin the host serves from. `'*'` is the fallback where that is unreadable:
 *  a lost message is worse than a broad target on loopback.
 */
function hostOrigin(): string {
  try {
    return location.origin || '*';
  } catch {
    return '*';
  }
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

/**
 * Ask the host to run one operation and wait for its answer.
 *
 * `timeoutMs` bounds the wait, so a host that never answers cannot leak a
 * pending entry for the life of the frame. The app is the one waiting, so a
 * slow host costs the app and never the shell.
 */
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
    window.parent.postMessage(message, hostOrigin());
  });
}

/** Tell the host something with no answer owed, e.g. a pong. */
export function tellHost(op: BridgeOp, args: unknown): void {
  const message: BridgeRequest = { type: BRIDGE_TYPE, id: '', op, args };
  window.parent.postMessage(message, hostOrigin());
}

/** The other direction: the host asking this frame to do something it can no
 *  longer reach into the frame to do itself. */
export const HOST_REQUEST_TYPE = 'lucidos:bridge:host';
export const HOST_REPLY_TYPE = 'lucidos:bridge:host:reply';

const hostHandlers = new Map<string, (args: unknown) => unknown>();
let serving = false;

/**
 * Answer one host request, for as long as this frame is up.
 *
 * The host used to reach through `contentWindow` for these: it read
 * `lucidos._capture` to screenshot an app, and drove `location.replace` to
 * switch apps and to deliver a fragment. An opaque origin denies all three, so
 * the frame does them and reports back.
 *
 * Only `window.parent` is answered, the same rule replies follow.
 */
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
      // No id means the host wants no answer, the shape a navigation takes.
      if (!data.id) return;
      window.parent.postMessage({
        type: HOST_REPLY_TYPE,
        id: data.id,
        ok,
        [ok ? 'value' : 'error']: payload,
      }, hostOrigin());
    };
    if (!run) return reply(false, `this app has no "${data.op}" handler`);
    void Promise.resolve()
      .then(() => run(data.args))
      .then((value) => reply(true, value), (e: unknown) => reply(false, String(e)));
  });
}

/** Test-only: forget the handlers and the one-shot install, so a case can set
 *  up a fresh frame. Not part of the runtime surface. */
export function _resetHostServingForTesting(): void {
  hostHandlers.clear();
  serving = false;
}

/** Subscribe to a push channel the host fans out on. Returns the unsubscribe. */
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

/**
 * Flatten a `HeadersInit` into a plain record.
 *
 * `HeadersInit` is three shapes: a record, a `Headers`, or an array of pairs.
 * Only the first survives a spread, so the other two used to vanish without a
 * word, an `Authorization` on a proxy call included. `new Headers` accepts all
 * three and is the one thing that normalises them.
 */
export function headersToRecord(init: HeadersInit | undefined): Record<string, string> {
  if (!init) return {};
  const out: Record<string, string> = {};
  new Headers(init).forEach((value, name) => { out[name] = value; });
  return out;
}

/** Flatten a `RequestInit` body into something structured clone can carry. */
export function toWireBody(body: BodyInit | null | undefined): WireBody {
  if (body === null || body === undefined) return { kind: 'none' };
  if (typeof body === 'string') return { kind: 'text', value: body };
  if (typeof FormData !== 'undefined' && body instanceof FormData) {
    return { kind: 'form', entries: [...body.entries()] as Array<[string, string | File]> };
  }
  if (body instanceof Blob || body instanceof ArrayBuffer) {
    return { kind: 'binary', value: body };
  }
  // `URLSearchParams` and the typed-array views both have a faithful string or
  // buffer form, so nothing is dropped in silence.
  if (typeof URLSearchParams !== 'undefined' && body instanceof URLSearchParams) {
    return { kind: 'text', value: body.toString() };
  }
  if (ArrayBuffer.isView(body)) {
    // Sliced by the VIEW's own window. A view onto part of a larger buffer is
    // the ordinary case, and copying the whole buffer would send bytes the
    // caller never offered.
    return {
      kind: 'binary',
      value: body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength) as ArrayBuffer,
    };
  }
  throw new TypeError('This body shape cannot cross the app bridge');
}

/** Statuses the `Response` constructor refuses to give a body to. A DELETE
 *  answering 204 is the one the SDK meets. */
const NULL_BODY_STATUS = new Set([101, 103, 204, 205, 304]);

/** Rebuild a `Response` from what the host sent back, so every SDK caller keeps
 *  reading `.json()` / `.text()` / `.blob()` exactly as it did. */
export function fromWireResponse(wire: WireResponse): Response {
  return new Response(NULL_BODY_STATUS.has(wire.status) ? null : wire.body, {
    status: wire.status,
    statusText: wire.statusText,
    headers: wire.headers,
  });
}
