/** Building a call's event log, for the suites that replay one.
 *
 *  Three of them do, and each grew its own copy of these four. They belong
 *  together: a call's shape is one thing, and a fixture that drifts from its
 *  neighbour is how a replay stops describing the same call.
 */
import type { StoredEvent, ThreadEvent } from '../thread-events';

/** The two `StoredEvent` fields the fold routes by, spelled out so an event
 *  literal can carry them: the engine stamps both onto the wire payload. */
export type Recorded = ThreadEvent & { _eventId?: string; request_event_id?: string };

/** One recorded event, stamped so `seq` also orders it in time. The fold sorts
 *  by `created` first, so a fixture with no timestamps would not exercise the
 *  ordering the grouping depends on. */
export function ev(seq: number, e: Recorded): readonly [number, StoredEvent] {
  const created = `2026-08-31T07:15:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created } as StoredEvent] as const;
}

/** Append one event to a log already built. */
export function put(events: Map<number, StoredEvent>, seq: number, e: Recorded): void {
  const [s, stored] = ev(seq, e);
  events.set(s, stored);
}

/** What the caller said, when the talker fielded it alone. */
export function heard(seq: number, text: string, session = 'sess-1'): readonly [number, StoredEvent] {
  return ev(seq, { type: 'SpokenMessageReceived', session_id: session, text });
}

/** What Lucidos said out loud. */
export function said(seq: number, text: string, session = 'sess-1'): readonly [number, StoredEvent] {
  return ev(seq, { type: 'SpokenReplyGenerated', session_id: session, text, interrupted: false });
}
