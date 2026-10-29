/**
 * The message shapes a document and the shared SSE worker exchange, plus the
 * pure pong aggregation both sides are tested against.
 *
 * Split out of `sseWorker.ts` because that file installs an `onconnect`
 * handler at import time and can only run inside a `SharedWorker`. Everything
 * here is data and pure functions, so the host, the SDK and the unit tests can
 * import it anywhere.
 */

import type { PongAnswer } from '../eventStream';
import { PRESENCE_OUTCOMES_BY_STRENGTH } from '../generated/engine-constants';

/** How long the worker waits for its ports to answer a `PresenceCheck`.
 *
 *  A port answers over `postMessage`, so a live one replies in single-digit
 *  milliseconds and the collection settles early. This bound only covers a port
 *  that never answers, a frozen background tab being the real case.
 *
 *  It must stay well inside the engine's own `DEADLINE_MS` (2000, in
 *  `crates/lucidos-engine/src/scheduler/push.rs`), or the aggregated pong
 *  arrives after the push decision it exists to inform. */
export const PONG_COLLECT_MS = 300;

/** Document to worker. */
export type FromPort =
  | {
      t: 'hello';
      /** True for a host shell, false for an app iframe. An app has no presence
       *  voice today and does not gain one by sharing a connection. */
      pongs: boolean;
      streamUrl: string;
      pongUrl: string;
    }
  | { t: 'pong'; notificationId: string; answer: PongAnswer }
  /** The page came back and wants a silent upstream restarted. Always
   *  answered with `checked`. */
  | { t: 'check' }
  /** The page has proof the upstream missed a frame and wants it reopened now,
   *  however recently it spoke. Answered by the `error` of that drop. */
  | { t: 'restart' }
  | { t: 'bye' };

/** Worker to document. */
export type ToPort =
  /** The worker holds this port but has no hello from it. Posted as it takes a
   *  port on, while the page's first hello is still in flight. Any later one
   *  means the worker lost that hello, usually to a relaunch, so the page says
   *  `hello` again. */
  | { t: 'introduce' }
  | { t: 'checked' }
  | { t: 'frame'; data: string }
  | { t: 'open'; lateJoin?: boolean }
  /** `silentMs` is set when the upstream stalled rather than errored. */
  | { t: 'error'; silentMs?: number };

/** Merge every attached document's answer into the one pong the engine expects.
 *
 *  Keeps the strongest outcome (`PRESENCE_OUTCOMES_BY_STRENGTH`, strongest
 *  first). So one document that handled the notification suppresses the push
 *  for the connection. The engine does the same across
 *  tabs on one device (`system-knowhow/notifications.md` §3). Doing it here
 *  keeps `expected_pong_count` equal to the open-connection count, so the
 *  engine neither stalls to its deadline nor decides early.
 *
 *  `device_id` is read from the first answer. Every document of one workspace
 *  in one browser profile shares it, being workspace-scoped localStorage.
 *
 *  Returns null for no answers, which means nothing to POST. */
export function aggregatePongAnswers(answers: PongAnswer[]): PongAnswer | null {
  if (answers.length === 0) return null;
  const outcome =
    PRESENCE_OUTCOMES_BY_STRENGTH.find((o) => answers.some((a) => a.outcome === o)) ?? 'not_shown';
  return { device_id: answers[0].device_id, outcome };
}
