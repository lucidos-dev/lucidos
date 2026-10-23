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
import type { ToPort } from './worker/protocol';

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
 *  nothing arrived for `silentMs`, so it counts as dead. */
export type StreamDrop =
  | { reason: 'error' }
  | { reason: 'stalled'; silentMs: number; transport: 'direct' | 'shared' };

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
   *  Never closes a live stream. A silent one is dead after
   *  `LATE_JOIN_STALL_AFTER_MS`. */
  revalidate(): StreamHealth;
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
  const worker = new SharedWorker(targets.workerUrl, { name: 'lucidos-sse' });
  const port = worker.port;

  port.onmessage = (event: MessageEvent) => {
    const msg = event.data as Partial<ToPort> & { data?: string };
    if (!msg || typeof msg !== 'object') return;
    if (msg.t === 'frame') handlers.onFrame(msg.data as string);
    else if (msg.t === 'open') handlers.onOpen();
    else if (msg.t === 'error') {
      handlers.onError(
        typeof msg.silentMs === 'number'
          ? { reason: 'stalled', silentMs: msg.silentMs, transport: 'shared' }
          : { reason: 'error' },
      );
    }
  };
  port.start();
  port.postMessage({
    t: 'hello',
    pongs: opts.pongs,
    streamUrl: targets.streamUrl,
    pongUrl: targets.pongUrl,
  });

  const sayBye = () => {
    try {
      port.postMessage({ t: 'bye' });
    } catch { /* already gone */ }
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
      if (canListen) removeEventListener('pagehide', onPageHide);
      sayBye();
      port.close();
    },
    ownsReconnect: true,
    submitPong: (notificationId, answer) => {
      port.postMessage({ t: 'pong', notificationId, answer });
    },
    revalidate: () => {
      port.postMessage({ t: 'check' });
      return 'checking';
    },
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
