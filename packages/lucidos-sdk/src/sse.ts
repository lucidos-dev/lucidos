import { apiBase } from './_fetch';
import { eventStreamTargets, openEventStream, type EventStream } from './eventStream';

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

/** Where this app's SDK reaches the engine. Derived per call rather than at
 *  module load, so `lucidos.configure({ baseUrl })` still takes effect. */
function targets() {
  return eventStreamTargets(apiBase());
}

function dispatch(eventType: string, data: unknown, raw: SseEvent) {
  const set = listeners.get(eventType);
  if (set) {
    for (const cb of set) cb(data, raw);
  }
}

/** Route one frame's `data` payload to its listeners.
 *
 *  The one place both transports converge: a direct frame and a relayed frame
 *  land here identically, which is what makes them indistinguishable to a
 *  listener. `eventStream.test.ts` pins the relay half of that. */
function handleFrame(data: string): void {
  try {
    const parsed = JSON.parse(data) as SseEvent;
    const outerType = parsed?.type;
    if (!outerType) return;

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
  } catch { /* malformed SSE data */ }
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

    return () => {
      set!.delete(callback);
      if (set!.size === 0) listeners.delete(eventType);
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
    stream = openEventStream(
      targets(),
      {
        onFrame: handleFrame,
        // An app has no resync to run and no status chrome to repaint, so both
        // are no-ops here. Whichever transport it got reconnects for it.
        onOpen: () => {},
        onError: () => {},
      },
      // An app has no presence voice, exactly as it has none today. It holds a
      // port and never answers a PresenceCheck, so the worker does not count it
      // among the documents it waits for.
      { pongs: false },
    );
  },

  /** Close the SSE connection. */
  disconnect(): void {
    if (stream) {
      stream.close();
      stream = null;
    }
  },
};
