
import {
  EVENT_STREAM_RETRY_MS,
  LATE_JOIN_STALL_AFTER_MS,
  postPong,
  startLiveness,
  type Liveness,
  type PongAnswer,
} from '../eventStream';
import { SSE_KEEPALIVE_EVENT } from '../generated/engine-constants';
import {
  aggregatePongAnswers,
  PONG_COLLECT_MS,
  type FromPort,
  type ToPort,
} from './protocol';

interface Client {
  port: MessagePort;
  pongs: boolean;
}

const clients = new Set<Client>();

let source: EventSource | null = null;
let liveness: Liveness | null = null;
let streamUrl = '';
let pongUrl = '';
let upstreamOpen = false;
let streamHello: string | null = null;
let generation: string | null | undefined;

function clearCollections(): void {
  for (const open of collecting.values()) clearTimeout(open.timer);
  collecting.clear();
}
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

function send(client: Client, msg: ToPort): void {
  try {
    client.port.postMessage(msg);
  } catch {
    clients.delete(client);
  }
}

function broadcast(msg: ToPort): void {
  for (const client of [...clients]) send(client, msg);
}

function closeUpstream(): void {
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  liveness?.stop();
  liveness = null;
  source?.close();
  source = null;
  upstreamOpen = false;
  streamHello = null;
  generation = undefined;
  clearCollections();
}

function ensureSource(): void {
  if (source || !streamUrl) return;
  const es = new EventSource(streamUrl);
  source = es;
  const watch = startLiveness((silentMs) => {
    dropUpstream({ silentMs });
    ensureSource();
  });
  liveness = watch;
  es.onmessage = (event: MessageEvent) => {
    if (source !== es) return;
    watch.touch();
    const data = event.data as string;
    if (data.includes('StreamHello')) {
      try {
        const frame = JSON.parse(data);
        if (frame?.type === 'StreamHello') {
          clearCollections();
          const value = frame.data?.restore_generation;
          generation = value === null || typeof value === 'string' ? value : undefined;
          streamHello = data;
        }
      } catch {
      }
    }
    maybeStartPongCollection(data);
    broadcast({ t: 'frame', data });
  };
  es.addEventListener(SSE_KEEPALIVE_EVENT, () => watch.touch());
  es.onopen = () => {
    if (source !== es) return;
    watch.touch();
    upstreamOpen = true;
    broadcast({ t: 'open' });
  };
  es.onerror = () => {
    if (source !== es) return;
    dropUpstream({});
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      if (clients.size > 0) ensureSource();
    }, EVENT_STREAM_RETRY_MS);
  };
}

function dropUpstream(drop: { silentMs?: number }): void {
  closeUpstream();
  broadcast({ t: 'error', ...drop });
}

function restartSilentUpstream(): void {
  const silentMs = liveness?.silentMs() ?? 0;
  if (silentMs >= LATE_JOIN_STALL_AFTER_MS) dropUpstream({ silentMs });
  ensureSource();
}

function restartUpstream(): void {
  dropUpstream({});
  ensureSource();
}


interface Collection {
  expected: Set<Client>;
  answers: Map<Client, PongAnswer>;
  generation: string | null;
  timer: ReturnType<typeof setTimeout>;
}

const collecting = new Map<string, Collection>();

function maybeStartPongCollection(data: string): void {
  if (!data.includes('PresenceCheck')) return;

  let parsed: { type?: string; data?: { notification_id?: string } };
  try {
    parsed = JSON.parse(data);
  } catch {
    return;
  }
  if (parsed?.type !== 'PresenceCheck') return;
  const id = parsed.data?.notification_id;
  if (typeof id !== 'string' || collecting.has(id)) return;

  const expected = new Set([...clients].filter((c) => c.pongs));
  if (expected.size === 0 || generation === undefined) return;

  const timer = setTimeout(() => settle(id), PONG_COLLECT_MS);
  collecting.set(id, { expected, answers: new Map(), timer, generation });
}

function collectPong(client: Client, notificationId: string, answer: PongAnswer, sourceGeneration: string | null): void {
  const open = collecting.get(notificationId);
  if (!open || !open.expected.has(client) || open.generation !== sourceGeneration) return;
  open.answers.set(client, answer);
  if (open.answers.size >= open.expected.size) settle(notificationId);
}

function settle(notificationId: string): void {
  const open = collecting.get(notificationId);
  if (!open) return;
  clearTimeout(open.timer);
  collecting.delete(notificationId);
  const merged = aggregatePongAnswers([...open.answers.values()]);
  if (merged) postPong(pongUrl, notificationId, merged, open.generation);
}


function attach(port: MessagePort): void {
  const client: Client = { port, pongs: false };

  port.onmessage = (event: MessageEvent) => {
    const msg = event.data as FromPort;
    if (!msg || typeof msg !== 'object') return;

    if (msg.t === 'hello') {
      client.pongs = msg.pongs;
      streamUrl ||= msg.streamUrl;
      pongUrl ||= msg.pongUrl;
      clients.add(client);
      restartSilentUpstream();
      if (upstreamOpen) {
        send(client, { t: 'open', lateJoin: true });
        if (streamHello !== null) send(client, { t: 'frame', data: streamHello });
      }
      return;
    }

    if (msg.t === 'check' || msg.t === 'restart') {
      if (!clients.has(client)) {
        send(client, { t: 'error' });
        send(client, { t: 'introduce' });
      } else if (msg.t === 'check') {
        restartSilentUpstream();
      } else {
        restartUpstream();
      }
      if (msg.t === 'check') send(client, { t: 'checked' });
      return;
    }

    if (msg.t === 'pong') {
      collectPong(client, msg.notificationId, msg.answer, msg.sourceGeneration);
      return;
    }

    if (msg.t === 'bye') {
      clients.delete(client);
      port.close();
      if (clients.size === 0) closeUpstream();
    }
  };

  port.start();
  send(client, { t: 'introduce' });
}

(self as unknown as SharedWorkerGlobalScope).onconnect = (event: MessageEvent) => {
  attach(event.ports[0]);
};
