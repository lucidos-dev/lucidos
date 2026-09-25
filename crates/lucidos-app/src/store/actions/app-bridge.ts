/**
 * The host half of the app bridge: what the shell does on an isolated app
 * frame's behalf.
 *
 * An app frame runs in its own renderer process so it cannot freeze the shell,
 * and the price is an opaque origin. Its own `fetch` to the engine is
 * CORS-blocked, `EventSource` with it, and storage throws. The SDK sends those
 * over `postMessage` and this answers them.
 *
 * The app half is `packages/lucidos-sdk/src/_bridge.ts`. The reasoning is in
 * `docs/plans/2026-09-19-an-app-frame-cannot-starve-the-shell.md`.
 */

import { appMayCall, normalizeSuffix } from '@lucidos/sdk';
import { API } from '../../api/client';
import { deviceIdHeader, readDeviceId } from '../../utils/deviceIdHeader';
import { registrationToAwait } from '../../utils/deviceRegistration';
import { appFrameFor, appIdForFrame } from '../../utils/appFrame';

/** Mirrors `packages/lucidos-sdk/src/_bridge.ts`. Two copies, because the SDK
 *  bundles standalone for app frames and cannot import the host. */
const BRIDGE_TYPE = 'lucidos:bridge';
const BRIDGE_REPLY_TYPE = 'lucidos:bridge:reply';
const BRIDGE_PUSH_TYPE = 'lucidos:bridge:push';

/**
 * The path this request will actually be sent to, or null if this app may not
 * make it at all.
 *
 * The answer is the engine's. `app_reach.rs` classifies every `/api/v1` route,
 * and a test there fails the build on a route that says nothing. The generated
 * table is what `appMayCall` reads (ADR 0231), and the host keeps no list of
 * its own: two would be one too many.
 *
 * Normalised FIRST, and the caller fetches what came back, so what was checked
 * and what is sent are the same string. Checking the raw path and sending it
 * separately is the whole bug class: the URL parser resolves a dot segment
 * afterwards, and it counts `%2e%2e` as one, so `/data/%2e%2e/credentials`
 * passes a literal `..` scan and lands on `/api/v1/credentials`.
 *
 * One rewrite happens after this, [`stampDeviceScope`], and it may touch a
 * query value only. The pathname it rebuilds from is the one checked here, so
 * the route cannot move. Anything else added between the check and the `fetch`
 * reopens the bug class above.
 *
 * `appId` is the calling app. Nothing reads it yet, and it is threaded through
 * so a per-app grant lands without re-plumbing the bridge.
 */
export function resolveBridgePath(
  path: string,
  method = 'GET',
  appId: string | null = null,
): string | null {
  const resolved = normalizeSuffix(path);
  if (!resolved) return null;
  return appMayCall(method, resolved.pathname, appId) ? resolved.full : null;
}

/** Whether the bridge may call this path at all. The resolver above is what
 *  the call itself goes through; this is for the tests and for a reader. */
export function pathIsAllowed(path: string, method = 'GET'): boolean {
  return resolveBridgePath(path, method) !== null;
}

/** What a frame sends when it means "the device I am running on".
 *  Mirrors `THIS_DEVICE` in `packages/lucidos-sdk/src/preferences.ts`. */
export const THIS_DEVICE = '@device';

/**
 * Put the real device id where the frame asked for its own.
 *
 * Theme, font and scale are device-scoped, so a preference read with no device
 * gets only the global rows. The frame cannot name the device, because the id
 * is the one thing the isolation keeps from it. So it names itself and the host
 * answers with the id it stamps on the header anyway.
 *
 * Only `/preferences`, which is the only read scoped this way. Widening it to
 * every path would hand the id to whatever an endpoint chooses to echo.
 */
function stampDeviceScope(path: string): string {
  const url = new URL(path, 'https://app.invalid');
  if (url.pathname !== '/preferences') return path;
  if (url.searchParams.get('device_id') !== THIS_DEVICE) return path;
  const id = readDeviceId();
  if (id) url.searchParams.set('device_id', id);
  else url.searchParams.delete('device_id');
  return `${url.pathname}${url.search}${url.hash}`;
}

/** Headers an app may not set on a bridged call.
 *
 *  `x-lucidos-*` is the engine's own namespace, and the device id inside it is
 *  what `api::actor` resolves the user from. The host stamps the real one after
 *  this, so letting an app supply one would let it name any device it liked. */
function sanitizeHeaders(headers: Record<string, string> | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [name, value] of Object.entries(headers ?? {})) {
    if (name.toLowerCase().startsWith('x-lucidos-')) continue;
    out[name] = value;
  }
  return out;
}

type WireBody =
  | { kind: 'none' }
  | { kind: 'text'; value: string }
  | { kind: 'binary'; value: ArrayBuffer | Blob }
  | { kind: 'form'; entries: Array<[string, string | File]> };

interface WireRequest {
  path: string;
  method: string;
  headers: Record<string, string>;
  body: WireBody;
  timeoutMs: number;
}

/** Rebuild the body the app flattened. `FormData` cannot cross a structured
 *  clone, so it arrived as its entries. */
function fromWireBody(body: WireBody): BodyInit | null {
  switch (body?.kind) {
    case 'text': return body.value;
    case 'binary': return body.value;
    case 'form': {
      const form = new FormData();
      for (const [name, value] of body.entries) form.append(name, value);
      return form;
    }
    default: return null;
  }
}

function headersToRecord(headers: Headers): Record<string, string> {
  const out: Record<string, string> = {};
  headers.forEach((value, name) => { out[name] = value; });
  return out;
}

/** The header naming the app a bridged call came from.
 *
 *  Stamped by the host, after [`sanitizeHeaders`] has dropped every
 *  `x-lucidos-*` the app supplied, so an app cannot name another app. The
 *  engine reads it to apply the same route table, which is defence in depth
 *  behind this check rather than the boundary itself (ADR 0231). */
export const APP_ID_HEADER = 'x-lucidos-app-id';

/** Make one engine call for an app, and flatten the answer for the wire. */
async function bridgedFetch(wire: WireRequest, appId: string | null): Promise<unknown> {
  const method = wire.method || 'GET';
  const resolved = resolveBridgePath(wire.path, method, appId);
  if (resolved === null) {
    throw new Error(`The app bridge does not carry ${method} ${wire.path}`);
  }
  const path = stampDeviceScope(resolved);
  const registration = registrationToAwait(`${API}${path}`, method);
  if (registration) await registration;
  const res = await fetch(`${API}${path}`, {
    method,
    headers: {
      ...sanitizeHeaders(wire.headers),
      ...deviceIdHeader(),
      ...(appId ? { [APP_ID_HEADER]: appId } : {}),
    },
    body: fromWireBody(wire.body),
    signal: AbortSignal.timeout(wire.timeoutMs || 30000),
  });
  return {
    status: res.status,
    statusText: res.statusText,
    headers: headersToRecord(res.headers),
    body: await res.arrayBuffer(),
  };
}

// ---------------------------------------------------------------------------
// The event stream, fanned out
// ---------------------------------------------------------------------------

/** App frames that asked for the stream, each beside the element it is. The
 *  shell holds ONE connection and relays to these, so ten open apps still cost
 *  one upstream. */
const sseSubscribers = new Map<MessageEventSource, HTMLIFrameElement>();

/** Post to one app frame. Its origin is opaque, so `'*'` is the only target
 *  that reaches it; the message goes to that window object and no other. */
function postToFrame(target: MessageEventSource, message: unknown): void {
  (target as Window).postMessage(message, '*');
}

/** Relay one thing to every subscribed frame, dropping any that has gone.
 *
 *  `isConnected` on the element kept at subscribe time, never a fresh DOM
 *  query. This runs once per event-stream frame, and a streaming turn is
 *  thousands of them: a `querySelectorAll` per frame per subscriber would put
 *  work back on the main thread this change exists to protect. */
function fanOut(payload: unknown): void {
  for (const [target, frame] of [...sseSubscribers]) {
    if (!frame.isConnected) {
      sseSubscribers.delete(target);
      continue;
    }
    postToFrame(target, { type: BRIDGE_PUSH_TYPE, channel: 'sse', data: payload });
  }
}

/** Push one thing to one app frame, on a named channel.
 *
 *  The fan-out above is for the event stream, where every subscriber gets the
 *  same payload. A renewed frame capability is per frame, so it goes straight
 *  to the element rather than through the subscriber map. */
export function postToAppFrame(
  frame: HTMLIFrameElement,
  channel: string,
  data: unknown,
): boolean {
  const target = frame.contentWindow;
  if (!target) return false;
  postToFrame(target, { type: BRIDGE_PUSH_TYPE, channel, data });
  return true;
}

/** One event-stream frame, verbatim, to every app that asked for it. */
export function fanOutEventFrame(data: string): void {
  if (sseSubscribers.size === 0) return;
  fanOut({ kind: 'frame', data });
}

/** The upstream opened or dropped. An app has no resync to run, but the SDK's
 *  handlers are the same shape as the shell's, so both arms are relayed. */
export function fanOutEventStreamStatus(kind: 'open' | 'error'): void {
  if (sseSubscribers.size === 0) return;
  fanOut({ kind });
}

// ---------------------------------------------------------------------------
// The other direction: asking a frame to act
// ---------------------------------------------------------------------------

const HOST_REQUEST_TYPE = 'lucidos:bridge:host';
const HOST_REPLY_TYPE = 'lucidos:bridge:host:reply';

let askCounter = 0;

/**
 * Ask one app frame to do something, and wait for its answer.
 *
 * The shell used to reach through `contentWindow` for these: reading
 * `lucidos._capture`, and driving `location.replace` to switch apps and deliver
 * a fragment. An isolated frame denies all three, so it does them for itself.
 *
 * `timeoutMs` bounds the wait, because the frame answering is the very thing
 * that may be wedged. A capture already had a deadline for that reason.
 */
export function tellFrame(frame: HTMLIFrameElement, op: string, args: unknown): boolean {
  const target = frame.contentWindow;
  if (!target) return false;
  // No id, so the frame owes no answer. A navigation has nothing useful to
  // report back: whether the document loaded is the frame's `load` event, not
  // a return value, exactly as `location.replace` never told the caller either.
  target.postMessage({ type: HOST_REQUEST_TYPE, op, args }, '*');
  return true;
}

export function askFrame(
  frame: HTMLIFrameElement,
  op: string,
  args: unknown,
  timeoutMs: number,
): Promise<unknown> {
  const target = frame.contentWindow;
  if (!target) return Promise.reject(new Error('the app frame has no browsing context'));
  const id = `host-${++askCounter}`;
  return new Promise((resolve, reject) => {
    const settle = (fn: () => void) => {
      window.removeEventListener('message', onReply);
      clearTimeout(timer);
      fn();
    };
    const timer = setTimeout(
      () => settle(() => reject(new Error(`the app did not answer "${op}" in ${timeoutMs}ms`))),
      timeoutMs,
    );
    const onReply = (event: MessageEvent) => {
      if (event.source !== target) return;
      const data = event.data as { type?: string; id?: string; ok?: boolean; value?: unknown; error?: string } | null;
      if (!data || data.type !== HOST_REPLY_TYPE || data.id !== id) return;
      settle(() => (data.ok
        ? resolve(data.value)
        : reject(new Error(data.error || `the app refused "${op}"`))));
    };
    window.addEventListener('message', onReply);
    target.postMessage({ type: HOST_REQUEST_TYPE, id, op, args }, '*');
  });
}

// ---------------------------------------------------------------------------
// Storage, on the app's behalf
// ---------------------------------------------------------------------------

/** Where an app's stored values live in the host's own storage.
 *
 *  The SDK already namespaces its keys by workspace, and the one thing an app
 *  stores (its scroll position) carries the app id as well. This prefix keeps
 *  the two key spaces apart, so nothing an app writes can land on a host key. */
const APP_STORAGE_PREFIX = 'appbridge:';

/** Mirror key, the same shape `_storage.ts` builds on the app side. */
function mirrorKey(key: string, session: boolean): string {
  return `${session ? 'session' : 'local'}:${key}`;
}

function appStorage(session: boolean): Storage | null {
  try {
    return session ? sessionStorage : localStorage;
  } catch {
    return null;
  }
}

/**
 * Everything this frame has stored, in one answer.
 *
 * One call rather than a read per key, because the SDK's readers are
 * synchronous and `postMessage` is not. The app mirrors this and serves its
 * reads from the mirror, so nothing downstream has to learn to wait.
 */
function storagePrime(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const session of [false, true]) {
    const store = appStorage(session);
    if (!store) continue;
    for (let i = 0; i < store.length; i++) {
      const raw = store.key(i);
      if (!raw?.startsWith(APP_STORAGE_PREFIX)) continue;
      const value = store.getItem(raw);
      // Keyed by store, matching `_storage.ts`'s `mirrorKey`. The two are
      // separate stores, and one map merging them would let a session write
      // answer a local read under the same name.
      if (value !== null) out[mirrorKey(raw.slice(APP_STORAGE_PREFIX.length), session)] = value;
    }
  }
  return out;
}

function storageSet(args: { key: string; value: string; session: boolean }): null {
  try {
    appStorage(args.session)?.setItem(`${APP_STORAGE_PREFIX}${args.key}`, args.value);
  } catch {
    // A full or disabled store costs an app its scroll position. Failing the
    // call would cost it the interaction that wrote it.
  }
  return null;
}

function storageRemove(args: { key: string; session: boolean }): null {
  try {
    appStorage(args.session)?.removeItem(`${APP_STORAGE_PREFIX}${args.key}`);
  } catch {
    /* nothing to clear in a store we cannot reach */
  }
  return null;
}

// ---------------------------------------------------------------------------
// The router
// ---------------------------------------------------------------------------

async function runOp(
  op: string,
  args: unknown,
  source: MessageEventSource,
  frame: HTMLIFrameElement,
): Promise<unknown> {
  switch (op) {
    case 'fetch': return bridgedFetch(args as WireRequest, appIdForFrame(frame));
    case 'sse.open': sseSubscribers.set(source, frame); return null;
    case 'sse.close': sseSubscribers.delete(source); return null;
    case 'storage.prime': return storagePrime();
    case 'storage.set': return storageSet(args as { key: string; value: string; session: boolean });
    case 'storage.remove': return storageRemove(args as { key: string; session: boolean });
    default: throw new Error(`The app bridge has no operation "${op}"`);
  }
}

/**
 * Answer app frames for as long as the shell is up.
 *
 * Two guards run before anything else. The message must carry the bridge's own
 * type, so the host's other frame protocols are untouched. The sender must be a
 * mounted app frame, checked against the live DOM by `contentWindow` identity.
 * An opaque origin cannot forge that, and `event.origin` cannot answer it.
 *
 * Every op is awaited and its outcome posted back. So a failure reaches the app
 * as a rejected SDK call, never as a promise that does not settle.
 */
export function installAppBridge(): () => void {
  const onMessage = (event: MessageEvent) => {
    const data = event.data as { type?: string; id?: string; op?: string; args?: unknown } | null;
    if (!data || typeof data !== 'object' || data.type !== BRIDGE_TYPE) return;
    const source = event.source;
    const frame = appFrameFor(source);
    if (!source || !frame) return;
    if (typeof data.op !== 'string') return;
    void runOp(data.op, data.args, source, frame).then(
      (value) => {
        if (!data.id) return;
        postToFrame(source, { type: BRIDGE_REPLY_TYPE, id: data.id, ok: true, value });
      },
      (error: unknown) => {
        if (!data.id) return;
        const message = error instanceof Error ? error.message : String(error);
        postToFrame(source, { type: BRIDGE_REPLY_TYPE, id: data.id, ok: false, error: message });
      },
    );
  };
  window.addEventListener('message', onMessage);
  return () => {
    window.removeEventListener('message', onMessage);
    sseSubscribers.clear();
  };
}

/** Test-only: forget every subscriber so module state cannot leak between
 *  cases. Not part of the runtime surface. */
export function _resetAppBridgeForTesting(): void {
  sseSubscribers.clear();
}
