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
  | { t: 'bye' };

/** Worker to document. */
export type ToPort =
  | { t: 'frame'; data: string }
  | { t: 'open'; lateJoin?: boolean }
  | { t: 'error' };

/** Merge every attached document's answer into the one pong the engine expects.
 *
 *  `is_active` and `event_in_viewport` are ORed, which is what the engine
 *  already does across tabs on one device (`system-knowhow/notifications.md`
 *  §3). Doing it here keeps `expected_pong_count` equal to the open-connection
 *  count, so the engine neither stalls to its deadline nor decides early.
 *
 *  `focused_thread_id` prefers an ACTIVE document's, because the engine reads
 *  it to decide whether the user is looking at the source thread. A hidden
 *  tab's focused thread says nothing about that.
 *
 *  `device_id` is read from the first answer. Every document of one workspace
 *  in one browser profile shares it, being workspace-scoped localStorage.
 *
 *  Returns null for no answers, which means nothing to POST. */
export function aggregatePongAnswers(answers: PongAnswer[]): PongAnswer | null {
  if (answers.length === 0) return null;
  const active = answers.filter((a) => a.is_active);
  const preferred = active.length > 0 ? active : answers;
  return {
    device_id: answers[0].device_id,
    is_active: active.length > 0,
    event_in_viewport: answers.some((a) => a.event_in_viewport),
    focused_thread_id:
      preferred.find((a) => a.focused_thread_id !== null)?.focused_thread_id ?? null,
  };
}
