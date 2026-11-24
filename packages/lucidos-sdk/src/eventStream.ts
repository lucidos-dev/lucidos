
import type { PRESENCE_OUTCOMES_BY_STRENGTH } from './generated/engine-constants';
import { SSE_KEEPALIVE_EVENT, SSE_KEEPALIVE_INTERVAL_MS } from './generated/engine-constants';
import type { FromPort, ToPort } from './worker/protocol';

export const EVENT_STREAM_RETRY_MS = 3000;

const STALL_SLACK_MS = 5000;

export const STREAM_STALL_AFTER_MS = 2 * SSE_KEEPALIVE_INTERVAL_MS + STALL_SLACK_MS;

export const LATE_JOIN_STALL_AFTER_MS = SSE_KEEPALIVE_INTERVAL_MS + STALL_SLACK_MS;

export interface Liveness {
  touch(): void;
  silentMs(): number;
  stop(): void;
}

export function startLiveness(
  onStall: (silentMs: number) => void,
  afterMs: number = STREAM_STALL_AFTER_MS,
): Liveness {
  let last = Date.now();
  let timer: ReturnType<typeof setTimeout> | null = null;
  const silentMs = () => Date.now() - last;
  const check = () => {
    const silent = silentMs();
    if (silent < afterMs) {
      timer = setTimeout(check, afterMs - silent);
      return;
    }
    timer = null;
    onStall(silent);
  };
  timer = setTimeout(check, afterMs);
  return {
    touch: () => {
      last = Date.now();
    },
    silentMs,
    stop: () => {
      if (timer) clearTimeout(timer);
      timer = null;
    },
  };
}

export type PresenceOutcome = (typeof PRESENCE_OUTCOMES_BY_STRENGTH)[number];

export interface PongAnswer {
  device_id: string;
  outcome: PresenceOutcome;
}

export type StreamDrop =
  | { reason: 'error' }
  | { reason: 'stalled'; silentMs: number; transport: 'direct' | 'shared' }
  | { reason: 'worker-restarted' }
  | { reason: 'worker-unresponsive' };

export interface EventStreamHandlers {
  onFrame(data: string): void;
  onOpen(): void;
  onError(drop: StreamDrop): void;
}

export interface EventStream {
  close(): void;
  readonly ownsReconnect: boolean;
  submitPong(notificationId: string, answer: PongAnswer): void;
  revalidate(): StreamHealth;
  restart(): void;
}

export type StreamHealth = 'live' | 'stale' | 'checking';

export interface EventStreamTargets {
  streamUrl: string;
  pongUrl: string;
  workerUrl: string;
}

export function eventStreamTargets(apiBase: string): EventStreamTargets {
  return {
    streamUrl: `${apiBase}/events`,
    pongUrl: `${apiBase}/presence-pong`,
    workerUrl: `${apiBase}/sse-worker.js`,
  };
}

class PongOrigin {
  generation: string | null | undefined;
  private changed = false;
  observe(data: string): void {
    try {
      const frame = JSON.parse(data);
      if (frame?.type !== 'StreamHello') return;
      const generation = frame.data?.restore_generation;
      if (generation !== null && typeof generation !== 'string') return;
      if (this.generation === undefined) this.generation = generation;
      else if (this.generation !== generation) this.changed = true;
    } catch {  }
  }
  get ready(): boolean { return this.generation !== undefined && !this.changed; }
}

export function postPong(
  pongUrl: string,
  notificationId: string,
  answer: PongAnswer,
  generation: string | null,
): void {
  void fetch(pongUrl, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'x-lucidos-restore-generation': generation ?? 'none' },
    body: JSON.stringify({ notification_id: notificationId, ...answer }),
    keepalive: true,
  }).catch((e) => {
    console.warn('[PresencePong] Failed to POST:', e);
  });
}

export function openDirectEventStream(
  targets: EventStreamTargets,
  handlers: EventStreamHandlers,
): EventStream {
  const origin = new PongOrigin();
  const es = new EventSource(targets.streamUrl);
  const liveness = startLiveness((silentMs) => {
    es.close();
    handlers.onError({ reason: 'stalled', silentMs, transport: 'direct' });
  });
  es.onmessage = (event: MessageEvent) => {
    liveness.touch();
    origin.observe(event.data as string);
    handlers.onFrame(event.data as string);
  };
  es.addEventListener(SSE_KEEPALIVE_EVENT, () => liveness.touch());
  es.onopen = () => {
    liveness.touch();
    handlers.onOpen();
  };
  es.onerror = () => handlers.onError({ reason: 'error' });
  return {
    close: () => {
      liveness.stop();
      es.close();
    },
    ownsReconnect: false,
    submitPong: (notificationId, answer) => {
      if (origin.ready) postPong(targets.pongUrl, notificationId, answer, origin.generation!);
    },
    revalidate: () => (liveness.silentMs() >= LATE_JOIN_STALL_AFTER_MS ? 'stale' : 'live'),
    restart: () => {
      liveness.stop();
      es.close();
      handlers.onError({ reason: 'error' });
    },
  };
}

export function sharedWorkerAvailable(): boolean {
  return typeof SharedWorker !== 'undefined';
}

export const WORKER_ANSWER_MS = 5000;

interface WorkerLink {
  worker: SharedWorker;
  port: MessagePort;
  heard: boolean;
}

export function openSharedEventStream(
  targets: EventStreamTargets,
  handlers: EventStreamHandlers,
  opts: { pongs: boolean },
): EventStream {
  const origin = new PongOrigin();
  const hello: FromPort = {
    t: 'hello',
    pongs: opts.pongs,
    streamUrl: targets.streamUrl,
    pongUrl: targets.pongUrl,
  };
  let workerAnswers = false;
  let answerTimer: ReturnType<typeof setTimeout> | null = null;
  const stopWaitingForAnswer = () => {
    if (answerTimer) clearTimeout(answerTimer);
    answerTimer = null;
  };

  const attach = (): WorkerLink => {
    const worker = new SharedWorker(targets.workerUrl, { name: 'lucidos-sse' });
    const link: WorkerLink = { worker, port: worker.port, heard: false };
    link.port.onmessage = (event: MessageEvent) => {
      const msg = event.data as ToPort | null;
      if (!msg || typeof msg !== 'object') return;
      const first = !link.heard;
      link.heard = true;
      stopWaitingForAnswer();
      if (msg.t === 'frame') {
        origin.observe(msg.data);
        handlers.onFrame(msg.data);
      } else if (msg.t === 'open') handlers.onOpen();
      else if (msg.t === 'error') {
        handlers.onError(
          typeof msg.silentMs === 'number'
            ? { reason: 'stalled', silentMs: msg.silentMs, transport: 'shared' }
            : { reason: 'error' },
        );
      } else if (msg.t === 'introduce') {
        workerAnswers = true;
        if (!first) {
          handlers.onError({ reason: 'worker-restarted' });
          link.port.postMessage(hello);
        }
      }
    };
    link.port.start();
    link.port.postMessage(hello);
    return link;
  };

  let link = attach();

  const sayBye = () => {
    try {
      link.port.postMessage({ t: 'bye' });
    } catch {  }
  };

  const reattach = () => {
    handlers.onError({ reason: 'worker-unresponsive' });
    sayBye();
    link.port.close();
    link = attach();
  };

  const askWorker = (msg: FromPort) => {
    link.port.postMessage(msg);
    if (!workerAnswers || answerTimer) return;
    const armedAt = Date.now();
    answerTimer = setTimeout(() => {
      answerTimer = null;
      if (Date.now() - armedAt >= 2 * WORKER_ANSWER_MS) askWorker({ t: 'check' });
      else reattach();
    }, WORKER_ANSWER_MS);
  };

  const onPageHide = (event: PageTransitionEvent) => {
    if (event.persisted) return;
    sayBye();
  };
  const canListen = typeof addEventListener === 'function';
  if (canListen) addEventListener('pagehide', onPageHide);

  return {
    close: () => {
      stopWaitingForAnswer();
      if (canListen) removeEventListener('pagehide', onPageHide);
      sayBye();
      link.port.close();
    },
    ownsReconnect: true,
    submitPong: (notificationId, answer) => {
      if (origin.ready) link.port.postMessage({ t: 'pong', notificationId, answer, sourceGeneration: origin.generation });
    },
    revalidate: () => {
      askWorker({ t: 'check' });
      return 'checking';
    },
    restart: () => askWorker({ t: 'restart' }),
  };
}

export function openEventStream(
  targets: EventStreamTargets,
  handlers: EventStreamHandlers,
  opts: { pongs: boolean },
): EventStream {
  if (sharedWorkerAvailable()) {
    try {
      return openSharedEventStream(targets, handlers, opts);
    } catch (err) {
      console.warn('[SSE] SharedWorker unavailable, opening a direct stream:', err);
    }
  }
  return openDirectEventStream(targets, handlers);
}
