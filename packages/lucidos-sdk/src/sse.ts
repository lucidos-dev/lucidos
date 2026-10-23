import { apiBase } from './_fetch';
import {
  EVENT_STREAM_RETRY_MS,
  eventStreamTargets,
  openEventStream,
  type EventStream,
} from './eventStream';
import { isBridged, tellHost } from './_bridge';
import { runIsolated } from './_isolate';
import { openBridgedEventStream } from './sseBridge';

export interface SseThreadEvent {
  type: 'ThreadEvent';
  data: {
    thread_id: string;
    event: { type: string; [key: string]: unknown };
    created: string;
    seq?: number;
    event_id: string;
  };
}

export interface SseSystemEvent {
  type: string;
  data: Record<string, unknown>;
}

export type SseEvent = SseThreadEvent | SseSystemEvent;

type SseCallback = (data: unknown, raw: SseEvent) => void;

let stream: EventStream | null = null;
const listeners = new Map<string, Set<SseCallback>>();
const openListeners = new Set<() => void>();
/** Whether the stream is up RIGHT NOW, so a late `onOpen` can run at once. */
let streamOpen = false;
/** Counts opens, so a late `onOpen` that a real open overtook runs only once. */
let openCount = 0;
/** A rebuild pending for a transport that does not retry for itself. */
let retryTimer: ReturnType<typeof setTimeout> | null = null;

/** The listened types the host last heard, so a change is sent once. */
let toldTypes = '';
let typesScheduled = false;

function listenedTypes(): string[] {
  return [...listeners.keys()].sort();
}

/** Tell the host which event types this document listens to, once per change
 *  and batched to a microtask. Only a bridged stream has a host to tell. */
function scheduleTypesUpdate(): void {
  if (typesScheduled || !stream || !isBridged()) return;
  typesScheduled = true;
  queueMicrotask(() => {
    typesScheduled = false;
    const types = listenedTypes();
    const key = types.join('\n');
    if (!stream || key === toldTypes) return;
    toldTypes = key;
    tellHost('sse.types', { types });
  });
}

function clearRetry(): void {
  if (retryTimer) clearTimeout(retryTimer);
  retryTimer = null;
}

/** Where this app's SDK reaches the engine. Derived per call rather than at
 *  module load, so `lucidos.configure({ baseUrl })` still takes effect. */
function targets() {
  return eventStreamTargets(apiBase());
}


function dispatch(eventType: string, data: unknown, raw: SseEvent) {
  const set = listeners.get(eventType);
  if (!set) return;
  for (const cb of set) runIsolated(() => cb(data, raw));
}

function handleOpen(): void {
  streamOpen = true;
  openCount++;
  for (const cb of [...openListeners]) runIsolated(cb);
}

/** Route one frame's `data` payload to its listeners.
 *
 *  The one place both transports converge: a direct frame and a relayed frame
 *  land here identically, which is what makes them indistinguishable to a
 *  listener. `eventStream.test.ts` pins the relay half of that. */
function handleFrame(data: string): void {
  let parsed: SseEvent | null;
  try {
    parsed = JSON.parse(data) as SseEvent | null;
  } catch {
    return; // malformed SSE data
  }
  const outerType = parsed?.type;
  if (!parsed || !outerType) return;

  if (outerType === 'ThreadEvent') {
    // Thread events: { type: "ThreadEvent", data: { thread_id, event: { type, ... } } }
    const threadEvent = parsed as SseThreadEvent;
    const innerType = threadEvent.data?.event?.type;
    if (innerType) {
      dispatch(innerType, threadEvent.data, parsed);
    }
    // Also dispatch to "ThreadEvent" listeners (for generic thread watchers)
    dispatch('ThreadEvent', threadEvent.data, parsed);
  } else {
    // System events: { type: "NotificationCreated", data: { ... } }
    dispatch(outerType, parsed.data ?? parsed, parsed);
  }

  // Wildcard listeners get the full raw envelope
  dispatch('*', parsed, parsed);
}

export const sse = {
  /**
   * Subscribe to a specific event type.
   *
   * Works for both thread events and system events — the SDK unwraps
   * the wire format so you subscribe by the inner event name:
   *
   *   lucidos.sse.on('NavigationRequested', (data) => { ... })
   *   lucidos.sse.on('NotificationCreated', (data) => { ... })
   *   lucidos.sse.on('*', (raw) => { ... })  // wildcard — all events
   *
   * Returns an unsubscribe function.
   */
  on(eventType: string, callback: SseCallback): () => void {
    let set = listeners.get(eventType);
    if (!set) {
      set = new Set();
      listeners.set(eventType, set);
    }
    set.add(callback);
    scheduleTypesUpdate();

    return () => {
      set!.delete(callback);
      if (set!.size === 0) {
        listeners.delete(eventType);
        scheduleTypesUpdate();
      }
    };
  },

  /**
   * Run `callback` each time the stream opens: the first open and every
   * reopen after a drop. Events that landed while the stream was down are not
   * replayed, so this is where an app re-reads its data.
   *
   * If the stream is already open, `callback` runs once on the next microtask.
   * So `lucidos.sse.onOpen(render)` alone covers first load and reconnect.
   *
   * A method of its own rather than an `on()` type, so no event can fire it.
   * Connects the stream. Returns an unsubscribe function.
   */
  onOpen(callback: () => void): () => void {
    openListeners.add(callback);
    sse.connect();
    if (streamOpen) {
      const seenOpens = openCount;
      queueMicrotask(() => {
        if (openCount === seenOpens && openListeners.has(callback)) runIsolated(callback);
      });
    }
    return () => {
      openListeners.delete(callback);
    };
  },

  /** Open the SSE connection to the Lucidos event stream.
   *
   *  Idempotent, and one connection fans out to every `on(...)` listener in
   *  this document. Given `SharedWorker`, the connection is shared with every
   *  other document of this workspace. Ten open apps then cost one stream
   *  rather than ten. */
  connect(): void {
    if (stream) return;
    clearRetry();
    let opened: EventStream | null = null;
    const handlers = {
      onFrame: handleFrame,
      onOpen: handleOpen,
      onError: () => {
        streamOpen = false;
        if (!opened || stream !== opened || opened.ownsReconnect) return;
        // A direct EventSource stops retrying for good on a response that is
        // not a stream, such as a 503 page mid-restart. So rebuild it, on the
        // schedule the shell keeps.
        opened.close();
        stream = null;
        retryTimer = setTimeout(() => {
          retryTimer = null;
          sse.connect();
        }, EVENT_STREAM_RETRY_MS);
      },
    };
    // An isolated frame can open neither transport below: both reach the engine
    // from an opaque origin, and CORS refuses that. The host relays instead,
    // off the one connection it already holds.
    if (isBridged()) {
      const types = listenedTypes();
      toldTypes = types.join('\n');
      stream = opened = openBridgedEventStream(handlers, types);
      return;
    }
    stream = opened = openEventStream(
      targets(),
      handlers,
      // An app has no presence voice, exactly as it has none today. It holds a
      // port and never answers a PresenceCheck, so the worker does not count it
      // among the documents it waits for.
      { pongs: false },
    );
  },

  /** Close the SSE connection. */
  disconnect(): void {
    streamOpen = false;
    toldTypes = '';
    clearRetry();
    if (stream) {
      stream.close();
      stream = null;
    }
  },
};
