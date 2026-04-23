import { getBaseUrl } from './_fetch';

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

let eventSource: EventSource | null = null;
const listeners = new Map<string, Set<SseCallback>>();

function dispatch(eventType: string, data: unknown, raw: SseEvent) {
  const set = listeners.get(eventType);
  if (set) {
    for (const cb of set) cb(data, raw);
  }
}

export const sse = {
  /**
   * Subscribe to a specific event type.
   *
   * Works for both thread events and system events — the SDK unwraps
   * the wire format so you subscribe by the inner event name:
   *
   *   cognos.sse.on('NavigationRequested', (data) => { ... })
   *   cognos.sse.on('NotificationCreated', (data) => { ... })
   *   cognos.sse.on('*', (raw) => { ... })  // wildcard — all events
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

  /** Open the SSE connection to the CognOS event stream. */
  connect(): void {
    if (eventSource) return;

    eventSource = new EventSource(`${getBaseUrl()}/api/events`);

    eventSource.onmessage = (event) => {
      try {
        const parsed = JSON.parse(event.data) as SseEvent;
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
    };
  },

  /** Close the SSE connection. */
  disconnect(): void {
    if (eventSource) {
      eventSource.close();
      eventSource = null;
    }
  },
};
