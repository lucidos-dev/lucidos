/**
 * The event stream, relayed by the host.
 *
 * A third `EventStream` implementation beside the direct `EventSource` and the
 * `SharedWorker` port. An isolated app frame can open neither: both reach the
 * engine from an opaque origin, and CORS refuses that. So the host relays the
 * frames off the connection it already holds.
 *
 * It lives here rather than in `eventStream.ts` because that module promises no
 * DOM access, the worker imports it, and this needs `window`. The interface is
 * the seam, and a relayed frame carries the same `data` string the direct path
 * reads. Nothing downstream can tell which transport delivered it.
 */

import { callHost, onHostPush, tellHost } from './_bridge';
import type { EventStream, EventStreamHandlers } from './eventStream';

/** What the host fans out on the `sse` channel. */
type Relayed =
  | { kind: 'frame'; data: string }
  | { kind: 'open' }
  | { kind: 'error' };

export function openBridgedEventStream(handlers: EventStreamHandlers): EventStream {
  const off = onHostPush('sse', (payload) => {
    const relayed = payload as Relayed;
    if (relayed?.kind === 'frame') handlers.onFrame(relayed.data);
    else if (relayed?.kind === 'open') handlers.onOpen();
    else if (relayed?.kind === 'error') handlers.onError();
  });

  // Fire and forget. The host answers with nothing, and a failed subscribe is
  // reported by the absence of frames rather than by a rejection nobody reads.
  void callHost('sse.open', {}).catch(() => {
    handlers.onError();
  });

  return {
    // The host owns the upstream and its retry. A consumer that reconnected on
    // its own would drop this subscription for no gain, the same reason the
    // shared-worker transport says true.
    ownsReconnect: true,
    close() {
      off();
      tellHost('sse.close', {});
    },
    // An app has no presence voice and never answers a `PresenceCheck`, which
    // is why `sse.connect` passes `pongs: false` for one. Nothing to submit.
    submitPong() {},
  };
}
