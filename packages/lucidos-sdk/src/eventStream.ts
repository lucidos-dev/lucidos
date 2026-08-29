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

/** One document's answer to a `PresenceCheck`, before aggregation. */
export interface PongAnswer {
  device_id: string;
  is_active: boolean;
  focused_thread_id: string | null;
  event_in_viewport: boolean;
}

export interface EventStreamHandlers {
  /** One frame's `data` payload, verbatim. */
  onFrame(data: string): void;
  /** The upstream is live. On a reconnect the consumer owes a resync. */
  onOpen(): void;
  /** The upstream dropped. The consumer owes a resync on the next open. */
  onError(): void;
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
   *  Direct: POSTs it straight away. Shared: sends it to the worker, which ORs
   *  it with its other ports' answers and POSTs exactly one pong. Keeping the
   *  submit behind the transport is what holds the engine's
   *  `expected_pong_count` equal to its open-connection count. */
  submitPong(notificationId: string, answer: PongAnswer): void;
}

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
  es.onmessage = (event: MessageEvent) => handlers.onFrame(event.data as string);
  es.onopen = () => handlers.onOpen();
  es.onerror = () => handlers.onError();
  return {
    close: () => es.close(),
    ownsReconnect: false,
    submitPong: (notificationId, answer) =>
      postPong(targets.pongUrl, notificationId, answer),
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
    const msg = event.data as { t?: string; data?: string };
    if (!msg || typeof msg !== 'object') return;
    if (msg.t === 'frame') handlers.onFrame(msg.data as string);
    else if (msg.t === 'open') handlers.onOpen();
    else if (msg.t === 'error') handlers.onError();
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
