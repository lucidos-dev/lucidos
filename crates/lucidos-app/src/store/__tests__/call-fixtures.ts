import type { StoredEvent, ThreadEvent } from '../thread-events';

export type Recorded = ThreadEvent & { _eventId?: string; request_event_id?: string };

export function ev(seq: number, e: Recorded): readonly [number, StoredEvent] {
  const created = `2026-08-31T07:15:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created } as StoredEvent] as const;
}

export function put(events: Map<number, StoredEvent>, seq: number, e: Recorded): void {
  const [s, stored] = ev(seq, e);
  events.set(s, stored);
}

export function heard(seq: number, text: string, session = 'sess-1'): readonly [number, StoredEvent] {
  return ev(seq, { type: 'SpokenMessageReceived', session_id: session, text });
}

export function said(seq: number, text: string, session = 'sess-1'): readonly [number, StoredEvent] {
  return ev(seq, { type: 'SpokenReplyGenerated', session_id: session, text, interrupted: false });
}

export function saidOver(
  seq: number,
  secs: number,
  text: string,
  session = 'sess-1',
): readonly [number, StoredEvent] {
  return ev(seq, {
    type: 'SpokenReplyGenerated',
    session_id: session,
    text,
    interrupted: false,
    spoken_secs_before: secs,
  });
}

