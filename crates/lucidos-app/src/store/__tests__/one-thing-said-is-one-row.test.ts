import { describe, expect, it } from 'vitest';
import { ev, heard, said } from './call-fixtures';
import { groupIntoExchanges } from '../thread-events';
import type { Exchange, StoredEvent } from '../thread-events';

const MSG = 'msg-1';

function starterText(exchange: Exchange): string {
  return (exchange.userEvent as { text?: string }).text ?? '';
}

function spokenSteps(exchange: Exchange): string[] {
  return exchange.steps
    .filter(s => s.event.type === 'SpokenReplyGenerated')
    .map(s => (s.event as { text: string }).text);
}

describe('the reported call, replayed by its real clock', () => {
  function theCall(): Map<number, StoredEvent> {
    return new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      ev(2, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'They asked for a release.' }),
      ev(3, {
        type: 'MessageReceived',
        text: "Let's release",
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: MSG,
      }),
      ev(20, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-1', request_event_id: MSG }),
      heard(26, 'Status'),
      heard(27, ', please'),
      said(28, 'Still'),
      said(29, "in it. I'm pulling the current threads together."),
      ev(34, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-2', request_event_id: MSG }),
    ]);
  }

  it('draws one bubble for one breath, on both sides', () => {
    const exchanges = groupIntoExchanges(theCall());
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'SpokenMessageReceived',
    ]);
    expect(starterText(exchanges[1])).toBe('Status, please');
    expect(spokenSteps(exchanges[1])).toEqual([
      "Still in it. I'm pulling the current threads together.",
    ]);
  });

  it('reads the caller above the answer to them, and the work below', () => {
    const exchanges = groupIntoExchanges(theCall());
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual(['ToolCalled']);
    expect(exchanges[1].steps.map(s => s.event.type)).toEqual([
      'SpokenReplyGenerated',
      'ToolCalled',
    ]);
  });
});

describe('what is NOT one thing said', () => {
  function twoRemarks(gapSecs: number): Exchange[] {
    return groupIntoExchanges(new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      heard(10, 'Status'),
      heard(10 + gapSecs, ', please'),
    ]));
  }

  it('keeps a real pause as two bubbles', () => {
    const exchanges = twoRemarks(20);
    expect(exchanges.map(starterText)).toEqual(['Status', ', please']);
  });

  it('joins a transcriber hiccup into one', () => {
    const exchanges = twoRemarks(1);
    expect(exchanges.map(starterText)).toEqual(['Status, please']);
  });

  it('never joins across a step the reader saw, however close the clock', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      ev(2, {
        type: 'MessageReceived',
        text: 'watch the build',
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: MSG,
      }),
      heard(10, 'Status'),
      ev(11, { type: 'ToolCalled', name: 'run_bash', args: {}, request_event_id: MSG }),
      heard(12, ', please'),
    ]));
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'SpokenMessageReceived',
      'SpokenMessageReceived',
    ]);
  });

  it('never joins across the answer to the first half', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      heard(10, 'Status'),
      said(11, 'One moment.'),
      heard(12, ', please'),
    ]));
    expect(exchanges.map(starterText)).toEqual(['Status', ', please']);
  });

  it('measures the gap between neighbours, as the engine does', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      heard(10, 'So what I'),
      heard(12, 'wanted to ask'),
      heard(14, 'you is'),
      heard(16, 'this.'),
    ]));
    expect(exchanges.map(starterText)).toEqual(['So what I wanted to ask you is this.']);
  });

  it('never joins two speakers', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      heard(10, "What's the status"),
      said(11, 'Nothing is waiting.'),
    ]));
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['SpokenMessageReceived']);
    expect(starterText(exchanges[0])).toBe("What's the status");
    expect(spokenSteps(exchanges[0])).toEqual(['Nothing is waiting.']);
  });
});

describe('what the reader never saw does not cut a sentence', () => {
  function delegatedMidSentence(): Exchange[] {
    return groupIntoExchanges(new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      heard(10, 'Just spawn the coding agent'),
      ev(11, { type: 'WorkDelegated', session_id: 'sess-1', reason: '' }),
      heard(12, ', please'),
    ]));
  }

  it('joins across the delegation the talker left behind', () => {
    expect(delegatedMidSentence().map(starterText))
      .toEqual(['Just spawn the coding agent, please']);
  });

  it('keeps the delegation on the sentence that prompted it', () => {
    const [utterance] = delegatedMidSentence();
    expect(utterance.steps.map(s => s.event.type)).toEqual(['WorkDelegated']);
    expect(utterance.tookTheTurn).toBe(true);
  });
});
