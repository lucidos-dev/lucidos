import { describe, it, expect } from 'vitest';
import { ev, heard, put, said } from './call-fixtures';
import { statusLabel } from '../exchange-status';
import { exchangeStatus, groupIntoExchanges, type StoredEvent } from '../thread-events';

const MSG = 'msg-1';

function lastExchange(events: Map<number, StoredEvent>) {
  const exchanges = groupIntoExchanges(events);
  return exchanges[exchanges.length - 1];
}

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

  it('hands a real turn in the same exchange to the ordinary machinery', () => {
    const events = theCall();
    put(events, 8, { type: 'ToolCalled', name: 'list_files', args: {} });
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, false)).toBe('streaming');
  });

  it('settles an unterminated turn on an idle thread without claiming an abort', () => {
    const events = new Map([
      ev(1, { type: 'MessageReceived', text: 'do the thing', mode: 'human', _eventId: MSG }),
      ev(2, { type: 'ToolCalled', name: 'list_files', args: {}, request_event_id: MSG }),
    ]);
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('done');
  });

  it('settles whatever a turn elsewhere on the thread is doing', () => {
    const exchange = lastExchange(theCall());
    expect(exchangeStatus(exchange, '', true, false, false, false)).toBe('done');
  });

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

  it('settles as soon as the talker answers', () => {
    const events = theCall();
    events.delete(7);
    put(events, 8, { type: 'SpokenReplyGenerated', session_id: 'sess-1', text: 'A good one. What are you building?', interrupted: false });
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('done');
  });
});

describe('a call in progress says so', () => {
  const REQUESTING = { label: 'Requesting', className: 'working' };

  function delegatedAndWaiting(): Map<number, StoredEvent> {
    return new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      said(2, 'Hi there. How can I help?'),
      heard(3, "What's going on in the codebase today?"),
      ev(4, {
        type: 'WorkDelegated',
        session_id: 'sess-1',
        reason: 'Check the workspace.',
        _eventId: MSG,
      }),
    ]);
  }

  it('keeps a delegated utterance live while its doer sleeps', () => {
    const exchange = lastExchange(delegatedAndWaiting());
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('streaming');
  });

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

  it('says "Requesting" and never "Working" before the doer wakes', () => {
    const exchange = lastExchange(delegatedAndWaiting());
    const status = exchangeStatus(exchange, '', true, false, false, true);
    expect(statusLabel(status,  false)).toEqual(REQUESTING);
  });

  it('does not let a hangup settle a question the doer never answered', () => {
    const events = delegatedAndWaiting();
    put(events, 5, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 12 });
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, false)).toBe('pending');
  });

  it('settles an unanswered delegated question once the thread idles', () => {
    const events = delegatedAndWaiting();
    put(events, 5, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 12 });
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('done');
  });

  it('leaves a delegated utterance to the ordinary machinery while the thread works', () => {
    const events = delegatedAndWaiting();
    put(events, 5, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 12 });
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, false)).not.toBe('done');
  });

  it('lets a hangup settle an utterance the talker held', () => {
    const events = new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      heard(2, 'What happened overnight?'),
      ev(3, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 8 }),
    ]);
    const exchange = lastExchange(events);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('done');
  });

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
    expect(exchangeStatus(exchange, '', true, false, false, false)).toBe('streaming');
  });
});

describe('a call that rang off after a failed turn settles', () => {
  function theReportedThread(): Map<number, StoredEvent> {
    return new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      ev(2, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'What are the best ideas' }),
      ev(3, {
        type: 'MessageReceived',
        text: 'What are the best ideas',
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: MSG,
      }),
      ev(4, { type: 'ResponseFailed', error: 'model_not_found', request_event_id: MSG }),
      heard(5, 'Again, please'),
      ev(6, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 54 }),
    ]);
  }

  it('reads done rather than spinning for ever', () => {
    const exchange = lastExchange(theReportedThread());
    expect(exchangeStatus(exchange, '', true, false, false, false)).toBe('done');
  });

  it('never shows the reported "Requesting" label', () => {
    const exchange = lastExchange(theReportedThread());
    const status = exchangeStatus(exchange, '', true, false, false, false);
    expect(statusLabel(status,  false).label).not.toBe('Requesting');
  });

  it('leaves the failed doer turn reading as an error', () => {
    const exchanges = groupIntoExchanges(theReportedThread());
    const doer = exchanges.find(e => e.userEvent.type === 'MessageReceived');
    expect(doer).toBeDefined();
    expect(exchangeStatus(doer!, '', false, false, false, false)).toBe('error');
  });
});
