/** A call that ended reads "Done", not "Aborted".
 *
 *  A call is not a turn (ADR 0148), so an exchange holding only spoken rows
 *  never gets a terminator. The stale detector reads steps with no terminator
 *  as a crashed turn. That is right for a turn and wrong for a call. Every
 *  finished call carried a red "Aborted" badge until the exchange said it was
 *  terminal by construction.
 *
 *  The detector itself must survive intact, so the second and third cases pin
 *  what it still catches.
 */
import { describe, it, expect } from 'vitest';
import { exchangeStatus, groupIntoExchanges, type StoredEvent, type ThreadEvent } from '../thread-events';

const MSG = 'msg-1';

type Recorded = ThreadEvent & { _eventId?: string; request_event_id?: string };

function ev(seq: number, e: Recorded): readonly [number, StoredEvent] {
  const created = `2026-08-30T18:24:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created } as StoredEvent] as const;
}

function heard(seq: number, text: string): readonly [number, StoredEvent] {
  return ev(seq, { type: 'SpokenMessageReceived', session_id: 'sess-1', text });
}

function said(seq: number, text: string): readonly [number, StoredEvent] {
  return ev(seq, { type: 'SpokenReplyGenerated', session_id: 'sess-1', text, interrupted: false });
}

/** The last exchange of a thread, which is the one the detector judges. */
function lastExchange(events: Map<number, StoredEvent>) {
  const exchanges = groupIntoExchanges(events);
  return exchanges[exchanges.length - 1];
}

/** The reported call, in the order the engine writes it now: the caller's
 *  words, then the reply to them. */
function theCall(): Map<number, StoredEvent> {
  return new Map([
    ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
    heard(2, 'Ask a question.'),
    said(3, "Of course. What's your question?"),
    heard(4, 'You are supposed to ask me a question.'),
    said(5, "Sure. What are you working on that you're excited about?"),
    heard(6, 'A user question'),
    ev(7, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 31 }),
  ]);
}

describe('a finished call is not aborted', () => {
  it('reads done once the thread is idle', () => {
    const exchange = lastExchange(theCall());
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('done');
  });

  it('still reads as a crash when a real turn died in the same exchange', () => {
    const events = theCall();
    // A delegated turn's work, with no terminator: the crash shape.
    events.set(8, {
      type: 'ToolCalled',
      name: 'list_files',
      args: {},
      created: '2026-08-30T18:24:08Z',
    } as StoredEvent);
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('aborted');
  });

  it('leaves an ordinary crashed turn alone', () => {
    const events = new Map([
      ev(1, { type: 'MessageReceived', text: 'do the thing', mode: 'human', _eventId: MSG }),
      ev(2, { type: 'ToolCalled', name: 'list_files', args: {}, request_event_id: MSG }),
    ]);
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('aborted');
  });

  it('does not answer for a doer turn still running elsewhere on the thread', () => {
    const exchange = lastExchange(theCall());
    // The thread is not idle, so the call falls through to the ordinary
    // machinery rather than declaring the thread finished.
    expect(exchangeStatus(exchange, '', true, false, false, false)).not.toBe('done');
  });

  // Voice never moves the thread's status, so `threadIdle` is true for the
  // whole of a talker-only call. Without the session's own end as the signal,
  // the very first spoken row would declare the live call finished.
  it('reads as live until the caller rings off', () => {
    const events = theCall();
    events.delete(7);
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('streaming');
  });

  it('reads as live again when a second call opens on the same thread', () => {
    const events = theCall();
    events.set(8, {
      type: 'VoiceSessionStarted',
      session_id: 'sess-2',
      created: '2026-08-30T18:25:08Z',
    } as StoredEvent);
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('streaming');
  });
});
