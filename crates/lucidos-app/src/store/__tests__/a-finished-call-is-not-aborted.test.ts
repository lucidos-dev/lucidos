/** A call that ended reads "Done", not "Aborted". A call still waiting on an
 *  answer reads "Requesting", not silence.
 *
 *  A call is not a turn (ADR 0148), so an exchange holding only spoken rows
 *  never gets a terminator. The stale detector reads steps with no terminator
 *  as a crashed turn. That is right for a turn and wrong for a call. Every
 *  finished call carried a red "Aborted" badge until the exchange said it was
 *  terminal by construction.
 *
 *  The detector itself must survive intact, so the second and third cases pin
 *  what it still catches.
 *
 *  The other half is the wait itself. An utterance whose doer has not woken
 *  holds no steps, and a stepless exchange on an idle thread read as finished.
 *  So no panel drew, and nothing said an answer was coming. See
 *  `docs/plans/2026-08-31-a-call-reads-as-one-conversation.md`.
 */
import { describe, it, expect } from 'vitest';
import { ev, heard, put, said } from './call-fixtures';
import { statusLabel } from '../exchange-status';
import { exchangeStatus, groupIntoExchanges, type StoredEvent } from '../thread-events';

const MSG = 'msg-1';

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
    put(events, 8, { type: 'ToolCalled', name: 'list_files', args: {} });
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
    put(events, 8, { type: 'VoiceSessionStarted', session_id: 'sess-2' });
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('streaming');
  });

  // Nothing is pending once the caller has heard back, so the shimmer stops
  // there rather than running to the end of the call.
  it('settles as soon as the talker answers', () => {
    const events = theCall();
    events.delete(7);
    put(events, 8, { type: 'SpokenReplyGenerated', session_id: 'sess-1', text: 'A good one. What are you building?', interrupted: false });
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('done');
  });
});

/** The gap between an utterance landing and an answer arriving. The reader is
 *  owed a progress affordance for all of it. It must not claim the doer is
 *  running before the doer has woken. */
describe('a call in progress says so', () => {
  const REQUESTING = { label: 'Requesting', className: 'working' };

  /** A caller's question the talker delegated, with the doer still asleep. Its
   *  only step is the delegation marker. */
  function delegatedAndWaiting(): Map<number, StoredEvent> {
    return new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      said(2, 'Hi there. How can I help?'),
      ev(3, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'Check the workspace.' }),
      ev(4, {
        type: 'MessageReceived',
        text: "What's going on in the codebase today?",
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: MSG,
      }),
    ]);
  }

  it('keeps a delegated utterance live while its doer sleeps', () => {
    const exchange = lastExchange(delegatedAndWaiting());
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('streaming');
  });

  // The talker stalls truthfully while it waits, and that stall is the FIRST
  // thing said on this path. Read as the answer it re-creates the silence: the
  // exchange settles "Done" seconds in, then flips back to Working when the
  // doer's first step lands.
  it('is not settled by the talker stalling for the doer', () => {
    const events = delegatedAndWaiting();
    put(events, 5, {
      type: 'SpokenReplyGenerated',
      session_id: 'sess-1',
      text: 'Let me check that for you.',
      interrupted: false,
    });
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('streaming');
  });

  it('keeps an utterance the talker has not answered live', () => {
    const events = new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      said(2, 'Hi there. How can I help?'),
      heard(3, 'What happened overnight?'),
    ]);
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('streaming');
  });

  // The label is what may overclaim, and this is the assertion that stops it.
  // A call-only exchange draws no step rows, so the ladder's `hasSteps` arm is
  // never taken and "Working" is unreachable before the doer wakes.
  it('says "Requesting" and never "Working" before the doer wakes', () => {
    const exchange = lastExchange(delegatedAndWaiting());
    const status = exchangeStatus(exchange, '', true, false, false, true);
    expect(statusLabel(status, /* hasSteps */ false)).toEqual(REQUESTING);
  });

  // Ringing off settles the call, not the question. A delegated one is
  // answered by the doer, and that answer outlives the call. A turn that
  // produced nothing before the hangup is the crash it looks like.
  it('does not let a hangup settle a question the doer never answered', () => {
    const events = delegatedAndWaiting();
    put(events, 5, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 12 });
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('aborted');
  });

  // The talker-only half of the same rule. Nobody else was asked, so ringing
  // off IS the end of it.
  it('lets a hangup settle an utterance the talker held', () => {
    const events = new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      heard(2, 'What happened overnight?'),
      ev(3, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 8 }),
    ]);
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('done');
  });

  // A typed thread must be untouched: an empty exchange on an idle thread is
  // finished, and only a call makes it a wait.
  it('leaves a stepless typed message finished', () => {
    const events = new Map([
      ev(1, { type: 'MessageReceived', text: 'do the thing', mode: 'human', _eventId: MSG }),
    ]);
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('done');
  });

  it('hands the exchange back once the doer starts working', () => {
    const events = delegatedAndWaiting();
    put(events, 5, { type: 'ToolCalled', name: 'list_files', args: {}, request_event_id: MSG });
    const exchange = lastExchange(events);
    // Not the call arm any more: a real step landed, so the ordinary machinery
    // owns the verdict and the stale detector is live again.
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('aborted');
  });
});
