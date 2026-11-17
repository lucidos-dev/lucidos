import { BRIDGE_PUSH_TYPE, HOST_REQUEST_TYPE, HOST_REPLY_TYPE } from '../../../../../packages/lucidos-sdk/src/_bridge';

// ---------------------------------------------------------------------------
// The event stream, fanned out
// ---------------------------------------------------------------------------

/** One app frame that asked for the stream: the element it is, and the event
 *  types it listens to. `null` takes every frame, as an SDK that names no
 *  types does. */
interface SseSubscriber {
  frame: HTMLIFrameElement;
  types: ReadonlySet<string> | null;
}

/** App frames that asked for the stream. The shell holds ONE connection and
 *  relays to these, so ten open apps still cost one upstream. */
const sseSubscribers = new Map<MessageEventSource, SseSubscriber>();

/** The listened types a frame sent, or `null` when it named none. */
function listenedTypes(args: unknown): ReadonlySet<string> | null {
  const types = (args as { types?: unknown } | null)?.types;
  if (!Array.isArray(types)) return null;
  return new Set(types.filter((t): t is string => typeof t === 'string'));
}

/** The event types a frame dispatches to in the SDK: a thread event's own type
 *  and `ThreadEvent`, else the system event's type. Empty when unreadable. */
export function frameEventTypes(data: string): string[] {
  let parsed: { type?: unknown; data?: { event?: { type?: unknown } } } | null;
  try {
    parsed = JSON.parse(data);
  } catch {
    return [];
  }
  const outer = parsed?.type;
  if (typeof outer !== 'string') return [];
  if (outer !== 'ThreadEvent') return [outer];
  const inner = parsed?.data?.event?.type;
  return typeof inner === 'string' ? [inner, outer] : [outer];
}

/** Whether a subscriber listens to a frame of these types. A `*` listener
 *  takes them all (ADR 0432). */
function wants(subscriber: SseSubscriber, types: () => readonly string[]): boolean {
  if (!subscriber.types || subscriber.types.has('*')) return true;
  return types().some((t) => subscriber.types!.has(t));
}

/** Whether the shell's stream is up RIGHT NOW, not whether it ever was.
 *
 *  A frame that subscribes to a live stream missed its `open`. So it gets one
 *  at once, as a port joining the shared worker does. Subscribing while the
 *  stream is down gets none: the app would re-read its data over a dead
 *  stream, then again on the real open. */
let hostStreamOpen = false;

/** Post to one app frame. Its origin is opaque, so `'*'` is the only target
 *  that reaches it; the message goes to that window object and no other. */
export function postToFrame(target: MessageEventSource, message: unknown): void {
  (target as Window).postMessage(message, '*');
}

/** Relay one thing to every subscribed frame, dropping any that has gone.
 *
 *  `isConnected` on the element kept at subscribe time, never a fresh DOM
 *  query. This runs once per event-stream frame, and a streaming turn is
 *  thousands of them: a `querySelectorAll` per frame per subscriber would put
 *  work back on the main thread this change exists to protect. */
function fanOut(payload: unknown, accepts: (subscriber: SseSubscriber) => boolean = () => true): void {
  for (const [target, subscriber] of [...sseSubscribers]) {
    if (!subscriber.frame.isConnected) {
      sseSubscribers.delete(target);
      continue;
    }
    if (accepts(subscriber)) postStreamTo(target, payload);
  }
}

function postStreamTo(target: MessageEventSource, payload: unknown): void {
  postToFrame(target, { type: BRIDGE_PUSH_TYPE, channel: 'sse', data: payload });
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

/** One event-stream frame, verbatim, to every app that listens to its type.
 *  The frame is parsed at most once, and only when a typed subscriber needs
 *  it: a streaming turn is thousands of frames. */
export function fanOutEventFrame(data: string): void {
  if (sseSubscribers.size === 0) return;
  let types: string[] | null = null;
  const frameTypes = () => (types ??= frameEventTypes(data));
  fanOut({ kind: 'frame', data }, (subscriber) => wants(subscriber, frameTypes));
}

/** The upstream opened or dropped. An app re-reads its data on each open
 *  (`lucidos.sse.onOpen`), since nothing replays what landed in the gap. */
export function fanOutEventStreamStatus(kind: 'open' | 'error'): void {
  hostStreamOpen = kind === 'open';
  if (sseSubscribers.size === 0) return;
  fanOut({ kind });
}

// ---------------------------------------------------------------------------
// The other direction: asking a frame to act
// ---------------------------------------------------------------------------

let askCounter = 0;

/**
 * Ask one app frame to do something, and wait for its answer.
 *
 * The shell used to reach through `contentWindow` for these: reading
 * `lucidos._capture`, and driving `location.replace` to deliver a fragment. An
 * isolated frame denies both, so it does them for itself.
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

export function subscribeAppEvents(source: MessageEventSource, frame: HTMLIFrameElement, args: unknown): void {
  sseSubscribers.set(source, { frame, types: listenedTypes(args) });
  if (hostStreamOpen) postStreamTo(source, { kind: 'open' });
}

export function updateAppEventTypes(source: MessageEventSource, args: unknown): void {
  const subscriber = sseSubscribers.get(source);
  if (subscriber) subscriber.types = listenedTypes(args);
}

export function unsubscribeAppEvents(source: MessageEventSource): void { sseSubscribers.delete(source); }
export function clearAppEventSubscribers(): void { sseSubscribers.clear(); }
export function resetAppEventStream(): void { sseSubscribers.clear(); hostStreamOpen = false; }
