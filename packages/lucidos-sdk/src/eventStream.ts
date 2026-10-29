/**
 * The event-stream transport seam.
 *
 * Every document used to own an `EventSource` to `/api/v1/events`, so
 * connection count scaled with the number of open apps. This module is the
 * layer that lets a document share one instead, without any consumer knowing
 * which transport delivered a frame.
 *
 * Two implementations satisfy `EventStream`: a direct `EventSource`, and a
 * `SharedWorker` port relaying one. A relayed frame carries the same `data`
 * string the direct path reads off `MessageEvent`, so the two are
 * indistinguishable downstream. That is the property the whole design rests on.
 *
 * No DOM access anywhere. The worker imports this file too, and a worker has no
 * `document`.
 */

import type { PRESENCE_OUTCOMES_BY_STRENGTH } from './generated/engine-constants';
import { SSE_KEEPALIVE_EVENT, SSE_KEEPALIVE_INTERVAL_MS } from './generated/engine-constants';
import type { FromPort, ToPort } from './worker/protocol';

/** How long a dropped stream waits before it opens again. The shell, the
 *  shared worker and an app's own stream all retry on this one schedule. */
export const EVENT_STREAM_RETRY_MS = 3000;

/** Slack on top of the engine's keepalive interval, for a keepalive delayed by
 *  a busy engine, a slow link or a throttled timer. */
const STALL_SLACK_MS = 5000;

/** How long a stream may stay silent before it counts as dropped: two missed
 *  keepalives, plus slack. A healthy stream is never silent this long, because
 *  the engine writes a keepalive every `SSE_KEEPALIVE_INTERVAL_MS`. */
export const STREAM_STALL_AFTER_MS = 2 * SSE_KEEPALIVE_INTERVAL_MS + STALL_SLACK_MS;

/** How long an upstream may stay silent before a late-joining port restarts it:
 *  one missed keepalive, plus slack. A joiner is a page that just came back,
 *  often from a freeze that left the stream half-open. */
export const LATE_JOIN_STALL_AFTER_MS = SSE_KEEPALIVE_INTERVAL_MS + STALL_SLACK_MS;

/** A running silence clock for one stream. */
export interface Liveness {
  /** Something arrived: an open, a frame or a keepalive. Resets the clock. */
  touch(): void;
  /** Milliseconds since the last `touch`, or since the clock started. */
  silentMs(): number;
  /** Stop the clock. Call it when the stream closes. */
  stop(): void;
}

/** Start a clock that calls `onStall` once, after `afterMs` of silence.
 *
 *  One timer re-arms for the remainder, so a busy stream costs a `Date.now()`
 *  per frame and no timer churn. A page frozen in the background resumes with
 *  its timer overdue, so a stream that died in the freeze trips at once. */
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

/** What a document already did with a `PresenceCheck` when it answered.
 *  Every value but `not_shown` means the reader has the notification in the
 *  app, so the engine suppresses the OS push. */
export type PresenceOutcome = (typeof PRESENCE_OUTCOMES_BY_STRENGTH)[number];

/** One document's answer to a `PresenceCheck`, before aggregation. */
export interface PongAnswer {
  device_id: string;
  outcome: PresenceOutcome;
}

/** Why a stream dropped. A stall is silence: the stream never errored, but
 *  nothing arrived for `silentMs`, so it counts as dead. The two `worker-`
 *  reasons are the shared worker itself failing this page: it forgot the
 *  page's hello, or it stopped answering. */
export type StreamDrop =
  | { reason: 'error' }
  | { reason: 'stalled'; silentMs: number; transport: 'direct' | 'shared' }
  | { reason: 'worker-restarted' }
  | { reason: 'worker-unresponsive' };

export interface EventStreamHandlers {
  /** One frame's `data` payload, verbatim. */
  onFrame(data: string): void;
  /** The upstream is live. On a reconnect the consumer owes a resync. */
  onOpen(): void;
  /** The upstream dropped. The consumer owes a resync on the next open. */
  onError(drop: StreamDrop): void;
}

export interface EventStream {
  close(): void;
  /** Whether the transport reconnects itself after a drop.
   *
   *  False for a direct `EventSource`: the shell deliberately tears its own
   *  down and rebuilds, because WebKit's native retry strands a resumed iOS
   *  PWA. True for the shared worker, which owns the one upstream and retries
   *  on the same schedule. A consumer that retried anyway would drop its port,
   *  and the last port leaving takes the worker's stream with it. */
  readonly ownsReconnect: boolean;
  /** Hand this document's answer to a `PresenceCheck` to the transport.
   *
   *  Direct: POSTs it straight away. Shared: sends it to the worker, which merges
   *  it with its other ports' answers and POSTs exactly one pong. Keeping the
   *  submit behind the transport is what holds the engine's
   *  `expected_pong_count` equal to its open-connection count. */
  submitPong(notificationId: string, answer: PongAnswer): void;
  /** Ask whether the upstream is still alive, for a page that just came back.
   *
   *  `live` and `stale` are answers now. `checking` means the transport asked
   *  its holder and will report a dead upstream through `onError`, then `onOpen`.
   *  A holder that does not answer within `WORKER_ANSWER_MS` is replaced, with
   *  the same two calls. Never closes a live stream. A silent one is dead after
   *  `LATE_JOIN_STALL_AFTER_MS`. */
  revalidate(): StreamHealth;
  /** Reopen the upstream now, for a caller holding proof it missed a frame.
   *
   *  Unlike `revalidate`, never waits for silence. Reports the drop through
   *  `onError`, then `onOpen` on the new upstream, so the consumer resyncs as
   *  after any other drop. A shared holder that does not answer is replaced,
   *  as for `revalidate`. */
  restart(): void;
}

/** The answer to `EventStream.revalidate`. */
export type StreamHealth = 'live' | 'stale' | 'checking';

/** Where a transport reaches the engine. All absolute, already carrying the
 *  workspace's `/<slug>` prefix, so nothing here re-derives a base path. */
export interface EventStreamTargets {
  /** `<base>/api/v1/events` */
  streamUrl: string;
  /** `<base>/api/v1/presence-pong` */
  pongUrl: string;
  /** `<base>/api/v1/sse-worker.js` */
  workerUrl: string;
}

/** Build the three targets from one already-versioned API base.
 *
 *  The SDK and the host reach that base differently (`apiUrl()` against a
 *  derived prefix, versus the host's `API` constant), but the suffixes are one
 *  contract. Naming them here keeps a renamed route from being updated on one
 *  side only, where the other would quietly fall back to a direct connection. */
export function eventStreamTargets(apiBase: string): EventStreamTargets {
  return {
    streamUrl: `${apiBase}/events`,
    pongUrl: `${apiBase}/presence-pong`,
    workerUrl: `${apiBase}/sse-worker.js`,
  };
}

/** POST a pong. Shared by the direct transport and the worker, so the two
 *  cannot drift on shape or on the `keepalive` flag.
 *
 *  `keepalive` so the pong still flushes if the document navigates away
 *  mid-send. Errors are swallowed to a warning: this runs without user intent,
 *  and the engine's deadline-then-push fallback covers a missed pong. */
export function postPong(
  pongUrl: string,
  notificationId: string,
  answer: PongAnswer,
): void {
  void fetch(pongUrl, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ notification_id: notificationId, ...answer }),
    keepalive: true,
  }).catch((e) => {
    console.warn('[PresencePong] Failed to POST:', e);
  });
}

/** Today's transport: this document owns the `EventSource`.
 *
 *  Kept as the fallback wherever `SharedWorker` is missing, which in practice
 *  means Chromium on Android and Android WebView. It is also what a popped-out
 *  app falls back to when the worker cannot be constructed. */
export function openDirectEventStream(
  targets: EventStreamTargets,
  handlers: EventStreamHandlers,
): EventStream {
  const es = new EventSource(targets.streamUrl);
  // A stream that goes quiet without erroring, such as a half-open connection
  // after a phone sleeps, never reaches `onerror`. Silence is the only signal.
  const liveness = startLiveness((silentMs) => {
    es.close();
    handlers.onError({ reason: 'stalled', silentMs, transport: 'direct' });
  });
  es.onmessage = (event: MessageEvent) => {
    liveness.touch();
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
    submitPong: (notificationId, answer) =>
      postPong(targets.pongUrl, notificationId, answer),
    revalidate: () => (liveness.silentMs() >= LATE_JOIN_STALL_AFTER_MS ? 'stale' : 'live'),
    restart: () => {
      liveness.stop();
      es.close();
      handlers.onError({ reason: 'error' });
    },
  };
}

/** Whether this browsing context can reach the shared holder at all.
 *
 *  Chromium on Android has never shipped `SharedWorker`, and neither has
 *  Android WebView. Everything else in our support set has it, Safari since
 *  16.4. A context without it takes `openDirectEventStream` and behaves exactly
 *  as it did before the stream was ever shared. */
export function sharedWorkerAvailable(): boolean {
  return typeof SharedWorker !== 'undefined';
}

/** How long a page waits for the shared worker to answer a `check` or a
 *  `restart`. A live worker answers over `postMessage` within milliseconds, so
 *  this only bounds a worker the browser has lost. */
export const WORKER_ANSWER_MS = 5000;

/** One attachment to the shared worker. The `SharedWorker` object is held for
 *  as long as its port is in use, so a collector cannot release it early. */
interface WorkerLink {
  worker: SharedWorker;
  port: MessagePort;
  /** A message arrived on this link. A worker asks a port to introduce itself
   *  before anything else, so an ask that arrives first is answered by the
   *  attach hello. A later ask comes from a relaunched instance. */
  heard: boolean;
}

/** Attach to the workspace's shared holder.
 *
 *  `pongs` declares whether this document answers a `PresenceCheck`. Only a
 *  host shell does. An app iframe holds a port and has no presence voice, which
 *  is exactly its position today.
 *
 *  Throws if the worker cannot be constructed. `openEventStream` catches that
 *  and falls back, so callers should prefer it. */
export function openSharedEventStream(
  targets: EventStreamTargets,
  handlers: EventStreamHandlers,
  opts: { pongs: boolean },
): EventStream {
  const hello: FromPort = {
    t: 'hello',
    pongs: opts.pongs,
    streamUrl: targets.streamUrl,
    pongUrl: targets.pongUrl,
  };
  /** A worker on this stream asked a port to introduce itself, so workers here
   *  answer a `check`. An older worker never asks and never answers, and other
   *  windows can keep one alive across a reload. */
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
      // Any message at all proves the worker is still there.
      stopWaitingForAnswer();
      if (msg.t === 'frame') handlers.onFrame(msg.data);
      else if (msg.t === 'open') handlers.onOpen();
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
    } catch { /* already gone */ }
  };

  /** Give up on a worker that stopped answering, and attach afresh. The
   *  browser launches a new instance if the old one is gone. Its `open` then
   *  runs the consumer's resync. */
  const reattach = () => {
    handlers.onError({ reason: 'worker-unresponsive' });
    sayBye();
    link.port.close();
    link = attach();
  };

  /** Post a message the worker always answers, and replace the worker if it
   *  stays silent. Until a worker here has asked, silence proves nothing. After
   *  a reattach it still counts, so a replacement that never runs is replaced
   *  too. */
  const askWorker = (msg: FromPort) => {
    link.port.postMessage(msg);
    if (!workerAnswers || answerTimer) return;
    const armedAt = Date.now();
    answerTimer = setTimeout(() => {
      answerTimer = null;
      // A timer a whole window late was frozen with the page, and the answer
      // may be queued behind it. Only a fresh window can tell.
      if (Date.now() - armedAt >= 2 * WORKER_ANSWER_MS) askWorker({ t: 'check' });
      else reattach();
    }, WORKER_ANSWER_MS);
  };

  // A document usually leaves WITHOUT calling `disconnect()`: a closed tab, a
  // navigation, an iframe removed from the DOM. The worker cannot notice on its
  // own, because posting to a dead port throws nothing. So an unannounced
  // departure would sit in its client set for the worker's whole life.
  //
  // A PERSISTED pagehide is bfcache, where this document and its port both
  // survive and resume with this same JS state. Saying goodbye there would
  // strand a page that is coming back.
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
      link.port.postMessage({ t: 'pong', notificationId, answer });
    },
    revalidate: () => {
      askWorker({ t: 'check' });
      return 'checking';
    },
    restart: () => askWorker({ t: 'restart' }),
  };
}

/** Open the best transport this context can reach.
 *
 *  Shared where possible, direct otherwise. The fallback also covers a worker
 *  that exists but cannot be constructed, which a strict CSP or a private
 *  browsing mode can cause. Failing to a working stream beats failing to none. */
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
