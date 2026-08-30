/** Every turn of a call reaches the transcript.
 *
 *  The call replayed here is the one in
 *  `docs/plans/2026-08-30-the-talker-sees-the-open-question.md`. Nine spoken
 *  turns happened and two rendered, and three independent gates each dropped
 *  their own share.
 *
 *  A resolved question divider suppressed its whole body, and on a voice
 *  thread that body is the call: the divider is an exchange boundary, so every
 *  spoken turn said while the card sat open folds in as its steps. The
 *  caller's own words had no render case at all. And a greeting said before
 *  anything started a turn landed with no exchange to go in.
 *
 *  No audio is kept, so a dropped spoken turn is gone rather than merely
 *  unrendered. That is what makes each of the three a real loss.
 */
import { describe, it, expect } from 'vitest';
import {
  dividerBodyIsSuppressed,
  exchangeResponseEvents,
  groupIntoExchanges,
  questionDividerResolution,
  type StoredEvent,
  type ThreadEvent,
} from '../thread-events';

const MSG = 'msg-1';

/** The two `StoredEvent` fields the fold routes by, spelled out so an event
 *  literal can carry them: the engine stamps both onto the wire payload. */
type Recorded = ThreadEvent & { _eventId?: string; request_event_id?: string };

function ev(seq: number, e: Recorded): readonly [number, StoredEvent] {
  const created = `2026-08-30T04:50:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created } as StoredEvent] as const;
}

function said(seq: number, text: string): readonly [number, StoredEvent] {
  return ev(seq, { type: 'SpokenReplyGenerated', session_id: 'sess-1', text, interrupted: false });
}

function heard(seq: number, text: string): readonly [number, StoredEvent] {
  return ev(seq, { type: 'SpokenMessageReceived', session_id: 'sess-1', text });
}

/** The whole call, in the order the engine wrote it. */
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

/** Every spoken line the call produced, in order. */
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
      // A spoken turn with no turn to land in becomes its own boundary, and
      // its initiator panel draws the words. Every other one is a step.
      const starter = exchange.userEvent as { type: string; text?: string };
      if (starter.type === 'SpokenReplyGenerated' || starter.type === 'SpokenMessageReceived') {
        drawn.push(starter.text ?? '');
      }
      for (const e of exchangeResponseEvents(exchange)) {
        if (e.type === 'spoken_reply' || e.type === 'spoken_message') drawn.push(e.text);
      }
    }
    expect(drawn).toEqual(SPOKEN);
  });

  it('opens a boundary for the greeting, which precedes every turn', () => {
    const exchanges = groupIntoExchanges(theCall());
    expect(exchanges[0].userEvent.type).toBe('SpokenReplyGenerated');
  });

  it('keeps the canceled divider drawing the call that happened under it', () => {
    const exchanges = groupIntoExchanges(theCall());
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked');
    if (!divider) throw new Error('the question opened no divider');

    // Resolved without an answer, which is what normally hides the body.
    expect(questionDividerResolution(divider)).toBe('canceled');
    // And five spoken rows sat in that body, so it renders anyway.
    expect(dividerBodyIsSuppressed(divider, exchangeResponseEvents(divider))).toBe(false);
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
