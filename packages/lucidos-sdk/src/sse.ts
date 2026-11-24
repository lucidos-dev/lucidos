import { apiBase } from './_fetch';
import { directRestoreGeneration } from './_restoreGeneration';
import {
  EVENT_STREAM_RETRY_MS,
  eventStreamTargets,
  openEventStream,
  type EventStream,
} from './eventStream';
import { isBridged, tellHost } from './_bridge';
import { runIsolated } from './_isolate';
import { openBridgedEventStream } from './sseBridge';
import { ROOT_THREAD_ROLE_ALIASES } from './generated/engine-constants';

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

const rootThreadAliases = new Map<string, string>([
  ...ROOT_THREAD_ROLE_ALIASES,
  ...ROOT_THREAD_ROLE_ALIASES.map(([legacy, current]) => [current, legacy] as const),
]);

let stream: EventStream | null = null;
const listeners = new Map<string, Set<SseCallback>>();
const openListeners = new Set<() => void>();
let streamOpen = false;
let openCount = 0;
let retryTimer: ReturnType<typeof setTimeout> | null = null;

let toldTypes = '';
let typesScheduled = false;

function listenedTypes(): string[] {
  const types = new Set(listeners.keys());
  for (const type of listeners.keys()) {
    const alias = rootThreadAliases.get(type);
    if (alias) types.add(alias);
  }
  return [...types].sort();
}

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

function targets() {
  return eventStreamTargets(apiBase());
}


function dispatch(eventType: string, data: unknown, raw: SseEvent) {
  const set = listeners.get(eventType);
  if (!set) return;
  for (const cb of set) runIsolated(() => cb(data, raw));
}

function handleOpen(): void {
  if (isBridged()) admitOpen();
}

function admitOpen(): void {
  streamOpen = true;
  openCount++;
  for (const cb of [...openListeners]) runIsolated(cb);
}

function handleFrame(data: string): void {
  let parsed: SseEvent | null;
  try {
    parsed = JSON.parse(data) as SseEvent | null;
  } catch {
    return;
  }
  const outerType = parsed?.type;
  if (!parsed || !outerType) return;

  if (outerType === 'StreamHello' && !isBridged()) {
    directRestoreGeneration.observe(parsed.data?.restore_generation);
    try { directRestoreGeneration.capture().assertCurrent(); }
    catch { streamOpen = false; return; }
    if (!streamOpen) admitOpen();
  }

  if (outerType === 'ThreadEvent') {
    const threadEvent = parsed as SseThreadEvent;
    const innerType = threadEvent.data?.event?.type;
    if (innerType) {
      dispatch(innerType, threadEvent.data, parsed);
      const alias = rootThreadAliases.get(innerType);
      if (alias) dispatch(alias, threadEvent.data, parsed);
    }
    dispatch('ThreadEvent', threadEvent.data, parsed);
  } else {
    dispatch(outerType, parsed.data ?? parsed, parsed);
  }

  dispatch('*', parsed, parsed);
}

export const sse = {
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

  onOpen(callback: () => void): () => void {
    openListeners.add(callback);
    sse.connect();
    if (streamOpen) {
      const seenOpens = openCount;
      queueMicrotask(() => {
        if (streamOpen && openCount === seenOpens && openListeners.has(callback)) runIsolated(callback);
      });
    }
    return () => {
      openListeners.delete(callback);
    };
  },

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
        opened.close();
        stream = null;
        retryTimer = setTimeout(() => {
          retryTimer = null;
          sse.connect();
        }, EVENT_STREAM_RETRY_MS);
      },
    };
    if (isBridged()) {
      const types = listenedTypes();
      toldTypes = types.join('\n');
      stream = opened = openBridgedEventStream(handlers, types);
      return;
    }
    stream = opened = openEventStream(
      targets(),
      handlers,
      { pongs: false },
    );
  },

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
