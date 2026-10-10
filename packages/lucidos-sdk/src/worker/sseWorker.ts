/**
 * The shared SSE holder: one `EventSource` per workspace, per browser profile.
 *
 * A `SharedWorker` is identified by its resolved script URL, and this script is
 * served under `/<slug>/api/v1/sse-worker.js`. Two workspaces therefore get two
 * workers with no keying code, and a document can never receive another
 * workspace's frames.
 *
 * The holder is deliberately not a document. A background tab can be frozen
 * while still holding a lock, which is what makes tab leader-election starve
 * foreground followers. A worker is not frozen that way, and it exits when its
 * last port goes.
 *
 * Frames are relayed verbatim, so a port cannot tell this apart from its own
 * `EventSource`. The one exception is `PresenceCheck`, which is relayed AND
 * aggregated; see `collectPong`.
 */

import { postPong, type PongAnswer } from '../eventStream';
import {
  aggregatePongAnswers,
  PONG_COLLECT_MS,
  type FromPort,
  type ToPort,
} from './protocol';

/** A connected document. `pongs` is false for an app iframe, which holds a
 *  port but has no presence voice, exactly as it has none today. */
interface Client {
  port: MessagePort;
  pongs: boolean;
}

const clients = new Set<Client>();

let source: EventSource | null = null;
let streamUrl = '';
let pongUrl = '';
/** Whether the upstream is up RIGHT NOW, not whether it ever was.
 *
 *  A port attaching to a live stream never sees the `open` that already fired.
 *  Without a synthetic one it would sit on "connecting" for good. Attaching
 *  while the stream is DOWN must not get one, or the document would paint
 *  connected over a dead upstream. */
let upstreamOpen = false;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

/** Matches the shell's own retry cadence, so a shared upstream recovers on the
 *  schedule a direct one did. */
const RECONNECT_MS = 3000;

/** Post to one client.
 *
 *  A port whose document has gone silently swallows the message. It does not
 *  throw, and there is no close event to listen for, so the worker cannot
 *  notice a departure on its own. The client announces one instead: on
 *  `disconnect()`, and on a non-persisted `pagehide`, which covers a closed
 *  tab, a navigation and a removed iframe.
 *
 *  What that leaves is a hard crash, and it is bounded. A stale entry only
 *  inflates the pong `expected` count, so collection waits its window instead
 *  of settling early. The browser then tears this worker down with its last
 *  real document.
 *
 *  The catch therefore covers an unserializable message, not a dead port. */
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

/** Drop the upstream and any pending retry. Called when the last document
 *  leaves, so the engine's connection count follows the documents. */
function closeUpstream(): void {
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  source?.close();
  source = null;
  upstreamOpen = false;
}

/** Open the upstream, once, on the first port that names it. */
function ensureSource(): void {
  if (source) return;
  const es = new EventSource(streamUrl);
  source = es;
  es.onmessage = (event: MessageEvent) => {
    const data = event.data as string;
    maybeStartPongCollection(data);
    broadcast({ t: 'frame', data });
  };
  es.onopen = () => {
    upstreamOpen = true;
    broadcast({ t: 'open' });
  };
  es.onerror = () => {
    // Relayed, not swallowed. Each port drives its own status chrome and arms
    // its own resync, so a follower can never read as connected while the
    // upstream is down.
    upstreamOpen = false;
    broadcast({ t: 'error' });

    // The worker owns the retry, and its ports must not duplicate it. A port
    // that tore itself down on this error would leave the worker, and the last
    // port leaving takes this very stream with it.
    es.close();
    source = null;
    if (reconnectTimer) clearTimeout(reconnectTimer);
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      if (clients.size > 0) ensureSource();
    }, RECONNECT_MS);
  };
}

// --- Presence aggregation -------------------------------------------------
//
// The engine waits for one pong per open SSE connection
// (`expected_pong_count`, crates/lucidos-engine/src/scheduler/push.rs). With
// one connection behind many documents, one pong is what it must get. So the
// worker asks its ponging ports, ORs their answers, and POSTs exactly once.

interface Collection {
  expected: number;
  answers: PongAnswer[];
  timer: ReturnType<typeof setTimeout>;
}

const collecting = new Map<string, Collection>();

/** Start collecting answers when a `PresenceCheck` goes out to the ports.
 *
 *  Parsing here rather than in each port keeps the decision to aggregate in one
 *  place. A malformed frame is simply not a PresenceCheck. */
function maybeStartPongCollection(data: string): void {
  // Reject on a substring before parsing. A coding-agent run streams a frame
  // per token, and each document parses the frame anyway. Parsing every one
  // here purely to read `type` would double the cost on the busiest path.
  // A false positive just falls through to the real check below.
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

  const expected = [...clients].filter((c) => c.pongs).length;
  if (expected === 0) return;

  const timer = setTimeout(() => settle(id), PONG_COLLECT_MS);
  collecting.set(id, { expected, answers: [], timer });
}

function collectPong(notificationId: string, answer: PongAnswer): void {
  const open = collecting.get(notificationId);
  if (!open) return;
  open.answers.push(answer);
  if (open.answers.length >= open.expected) settle(notificationId);
}

/** POST the one aggregated pong for this notification, and stop collecting. */
function settle(notificationId: string): void {
  const open = collecting.get(notificationId);
  if (!open) return;
  clearTimeout(open.timer);
  collecting.delete(notificationId);
  const merged = aggregatePongAnswers(open.answers);
  if (merged) postPong(pongUrl, notificationId, merged);
}

// --- Port lifecycle -------------------------------------------------------

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
      ensureSource();
      // A port attaching to a stream that is already open gets its `open` now.
      // It never saw the upstream's own, and without one it would sit on
      // "connecting" for good and never run its late-join reconcile.
      if (upstreamOpen) send(client, { t: 'open', lateJoin: true });
      return;
    }

    if (msg.t === 'pong') {
      collectPong(msg.notificationId, msg.answer);
      return;
    }

    if (msg.t === 'bye') {
      clients.delete(client);
      port.close();
      // The last document leaving releases the upstream, so an explicit
      // `sse.disconnect()` frees the engine's connection slot exactly as a
      // direct `EventSource` would. Left open, it would keep counting toward
      // `expected_pong_count` with nobody behind it to answer.
      if (clients.size === 0) closeUpstream();
    }
  };

  port.start();
}

(self as unknown as SharedWorkerGlobalScope).onconnect = (event: MessageEvent) => {
  attach(event.ports[0]);
};
