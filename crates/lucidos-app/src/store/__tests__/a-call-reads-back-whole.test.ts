import { describe, it, expect } from 'vitest';
import { ev, heard, put, said } from './call-fixtures';
import {
  changePanelHasContinuation,
  dividerBodyIsSuppressed,
  exchangeResponseEvents,
  groupIntoExchanges,
  questionDividerResolution,
  type StoredEvent,
} from '../thread-events';

const MSG = 'msg-1';

function theCall(): Map<number, StoredEvent> {
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
    said(5, 'I can check that for you.'),
    ev(6, { type: 'TextStreamed', text: 'Busy night. Sixty commits.', request_event_id: MSG }),
    ev(7, {
      type: 'ToolCalled',
      name: 'ask_user_question',
      args: {},
      _eventId: 'tc-1',
      request_event_id: MSG,
    }),
    ev(8, {
      type: 'UserQuestionAsked',
      tool_use_id: 'tu-0',
      cc_session_id: '',
      question: 'The mobile-webkit tail has no verdict. Do something now?',
      options: [{ id: 'opt-0', label: 'Run the tail now' }],
    }),
    ev(9, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 71 }),
    ev(10, { type: 'VoiceSessionStarted', session_id: 'sess-2' }),
    said(11, 'The codebase is clean right now.'),
    heard(12, 'What happened?'),
    said(13, 'Nothing urgent needs your attention.'),
    heard(14, 'Anything that needs me?'),
    ev(15, { type: 'UserQuestionAnswered', tool_use_id: 'tu-0', answer: { kind: 'Canceled' } }),
    ev(16, {
      type: 'ResponseCanceled',
      text: '',
      cause: 'user_stop',
      request_event_id: MSG,
    }),
    said(17, 'I did not quite finish answering that.'),
  ]);
}

const SPOKEN = [
  'Hi there. How can I help?',
  'I can check that for you.',
  'The codebase is clean right now.',
  'What happened?',
  'Nothing urgent needs your attention.',
  'Anything that needs me?',
  'I did not quite finish answering that.',
];

describe('a call reads back whole', () => {
  it('folds every spoken turn into an exchange, none dropped', () => {
    const exchanges = groupIntoExchanges(theCall());
    const drawn: string[] = [];
    for (const exchange of exchanges) {
      const starter = exchange.userEvent as { type: string; text?: string };
      if (starter.type === 'SpokenReplyGenerated' || starter.type === 'SpokenMessageReceived') {
        drawn.push(starter.text ?? '');
      }
      for (const e of exchangeResponseEvents(exchange)) {
        if (e.type === 'spoken_reply') drawn.push(e.text);
      }
    }
    expect(drawn).toEqual(SPOKEN);
  });

  it('opens a boundary for every caller utterance, delegated or not', () => {
    const exchanges = groupIntoExchanges(theCall());
    const asked = exchanges
      .map(e => e.userEvent as { type: string; text?: string })
      .filter(e => e.type === 'MessageReceived' || e.type === 'SpokenMessageReceived')
      .map(e => e.text ?? '');
    expect(asked).toEqual([
      "What's going on in the codebase today?",
      'What happened?',
      'Anything that needs me?',
    ]);
  });

  it('leaves the resolved divider above the utterances that followed it', () => {
    const types = groupIntoExchanges(theCall()).map(e => e.userEvent.type);
    expect(types).toEqual([
      'SpokenReplyGenerated',
      'MessageReceived',
      'UserQuestionAsked',
      'SpokenMessageReceived',
      'SpokenMessageReceived',
    ]);
  });

  it('holds the divider above a DELEGATED utterance too', () => {
    const events = theCall();
    events.delete(12);
    events.delete(14);
    put(events, 12, {
      type: 'MessageReceived',
      text: 'What happened?',
      mode: 'human',
      channel: 'chat',
      voice_session_id: 'sess-2',
      _eventId: 'msg-2',
    });
    put(events, 14, {
      type: 'MessageReceived',
      text: 'Anything that needs me?',
      mode: 'human',
      channel: 'chat',
      voice_session_id: 'sess-2',
      _eventId: 'msg-3',
    });
    const types = groupIntoExchanges(events).map(e => e.userEvent.type);
    expect(types).toEqual([
      'SpokenReplyGenerated',
      'MessageReceived',
      'UserQuestionAsked',
      'MessageReceived',
      'MessageReceived',
    ]);
  });

  it('opens a boundary for the greeting, which precedes every turn', () => {
    const exchanges = groupIntoExchanges(theCall());
    expect(exchanges[0].userEvent.type).toBe('SpokenReplyGenerated');
  });

  it('keeps the canceled divider drawing the call that happened under it', () => {
    const exchanges = groupIntoExchanges(theCall());
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked');
    if (!divider) throw new Error('the question opened no divider');

    expect(questionDividerResolution(divider)).toBe('canceled');
    expect(dividerBodyIsSuppressed(divider, exchangeResponseEvents(divider))).toBe(false);
  });

  it('keeps a change banner drawing a call that landed under it', () => {
    for (const banner of ['ChangeApplied', 'ChangeApplyFailed'] as const) {
      const events = new Map([
        ev(1, { type: 'MessageReceived', text: 'apply it', mode: 'human', _eventId: MSG }),
        ev(2, { type: 'ResponseGenerated', text: 'done', request_event_id: MSG }),
        ev(3, { type: banner, change_id: 'ch-1' }),
        ev(4, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
        said(5, 'Hi there. How can I help?'),
      ]);
      const panel = groupIntoExchanges(events).find(e => e.userEvent.type === banner);
      if (!panel) throw new Error(`${banner} opened no panel`);
      expect(panel.steps.map(s => s.event.type)).toContain('SpokenReplyGenerated');
      expect(changePanelHasContinuation(panel)).toBe(true);
    }
  });

  it('still hides a change banner with nothing under it', () => {
    const events = new Map([
      ev(1, { type: 'MessageReceived', text: 'apply it', mode: 'human', _eventId: MSG }),
      ev(2, { type: 'ResponseGenerated', text: 'done', request_event_id: MSG }),
      ev(3, { type: 'ChangeApplied', change_id: 'ch-1' }),
    ]);
    const panel = groupIntoExchanges(events).find(e => e.userEvent.type === 'ChangeApplied')!;
    expect(changePanelHasContinuation(panel)).toBe(false);
  });

  it('still hides a typed thread canceled divider, which has nothing to draw', () => {
    const events = new Map([
      ev(1, { type: 'MessageReceived', text: 'do the thing', mode: 'human', _eventId: MSG }),
      ev(2, {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu-1',
        cc_session_id: '',
        question: 'Which one?',
        options: [{ id: 'opt-0', label: 'This one' }],
      }),
      ev(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Canceled' } }),
    ]);
    const divider = groupIntoExchanges(events).find(e => e.userEvent.type === 'UserQuestionAsked');
    if (!divider) throw new Error('the question opened no divider');
    expect(questionDividerResolution(divider)).toBe('canceled');
    expect(dividerBodyIsSuppressed(divider, exchangeResponseEvents(divider))).toBe(true);
  });
});
