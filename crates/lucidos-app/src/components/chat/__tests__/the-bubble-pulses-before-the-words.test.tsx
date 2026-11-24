import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { chatExchangePropsEqual, describeInitiator, liveRowDrawsNoPanel } from '../ChatExchange';
import { isUserBubbleEvent } from '../../../store/thread-events/exchange';
import { vnodeToText } from './vnodeToText';
import { HEARING_YOU } from '../../../voice/callState';
import { exchangeStatus } from '../../../store/thread-events';
import type { Exchange, StoredEvent } from '../../../store/thread-events';

const here: string = dirname(fileURLToPath(import.meta.url));
const chatExchangeSource: string = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');
const voiceCallCss: string = readFileSync(
  resolve(here, '../../../styles/chat/voice-call.css'),
  'utf-8',
);
const partsSource: string = readFileSync(resolve(here, '../chat-exchange-parts.tsx'), 'utf-8');

const theRow: Exchange = {
  userEvent: {
    type: 'MessageReceived',
    text: '',
    _eventId: 'live-utterance:t:1',
    _liveUtterance: true,
    channel: 'chat',
  } as StoredEvent,
  userSeq: Number.MAX_SAFE_INTEGER,
  steps: [],
};

describe('the caller mid-sentence', () => {
  it('draws the ordinary user bubble, on the reader\'s own side', () => {
    const initiator = describeInitiator(theRow, '', [], 'tid');
    expect(initiator.variant).toBe('user');
    expect(initiator.label).toBe('You');
    expect(isUserBubbleEvent(theRow.userEvent)).toBe(true);
  });

  it('marks the bubble as spoken, the same mark the words will carry', () => {
    const initiator = describeInitiator(theRow, '', [], 'tid');
    expect(vnodeToText(initiator.status)).toContain('Spoken');
  });

  it('says what it is doing, for a reader who cannot see the pulse', () => {
    const initiator = describeInitiator(theRow, '', [], 'tid');
    const body = vnodeToText(initiator.details);
    expect(body).toContain(`<span class="visually-hidden">${HEARING_YOU}</span>`);
    expect(body).toContain('live-speech-bar');
  });

  it('never says "recording", because no audio is kept', () => {
    expect(vnodeToText(describeInitiator(theRow, '', [], 'tid').details).toLowerCase())
      .not.toContain('record');
  });
});

describe('nothing is drawn under it', () => {
  it('is excluded from the response panel, like a queued message is', () => {
    const line = chatExchangeSource
      .split('\n')
      .find(l => l.includes('const showResponsePanel'));
    expect(line).toBeDefined();
    expect(line).toContain('!isLiveRow');
  });

  it('draws a panel for the final words and none for a partial', () => {
    expect(liveRowDrawsNoPanel({ ...theRow.userEvent, text: 'the tests' } as StoredEvent)).toBe(false);
    expect(liveRowDrawsNoPanel({
      ...theRow.userEvent,
      text: 'the tes',
      _livePartial: true,
    } as StoredEvent)).toBe(true);
    expect(liveRowDrawsNoPanel(theRow.userEvent)).toBe(true);
  });

  it('excludes the talker\'s live row too', () => {
    expect(liveRowDrawsNoPanel({
      type: 'SpokenReplyGenerated',
      session_id: '',
      text: 'On it',
      interrupted: false,
      _eventId: 'live-reply:t:1',
      _liveReply: true,
    } as StoredEvent)).toBe(true);
  });
});

describe('the caller\'s words, before the engine has written them down', () => {
  const spoken: Exchange = {
    ...theRow,
    userEvent: { ...theRow.userEvent, text: 'fix the blank thread' } as StoredEvent,
  };

  it('draws the words, not the mark', () => {
    const body = vnodeToText(describeInitiator(spoken, '<p>fix the blank thread</p>', [], 'tid').details);
    expect(body).toContain('fix the blank thread');
    expect(body).not.toContain('live-speech-bar');
  });

  it('keeps the spoken mark, so it still reads as speech', () => {
    const initiator = describeInitiator(spoken, '<p>fix the blank thread</p>', [], 'tid');
    expect(vnodeToText(initiator.status)).toContain('Spoken');
    expect(initiator.variant).toBe('user');
  });

  it('reads as settled once the caller has finished saying it', () => {
    expect(exchangeStatus(spoken, '', true, false, false,  true, false))
      .toBe('done');
  });

  it('still reads as pending while the caller is speaking', () => {
    expect(exchangeStatus(theRow, '', true, false, false,  true, false))
      .toBe('pending');
  });

  it('still reads as pending on a partial', () => {
    const partial: Exchange = {
      ...theRow,
      userEvent: { ...theRow.userEvent, text: 'fix the blank', _livePartial: true } as StoredEvent,
    };
    expect(exchangeStatus(partial, '', true, false, false,  true, false))
      .toBe('pending');
  });

  it('re-renders when the pulse becomes the words', () => {
    const props = (exchange: Exchange) => ({
      exchange,
      revision: 0,
      threadId: 'tid',
      isLast: false,
      streamingBuffer: '',
    } as unknown as Parameters<typeof chatExchangePropsEqual>[0]);
    expect(chatExchangePropsEqual(props(theRow), props(spoken))).toBe(false);
  });
});

describe('the talker\'s bubble while it is being said', () => {
  const props = (exchange: Exchange) => ({
    exchange,
    revision: 0,
    threadId: 'tid',
    isLast: false,
    streamingBuffer: '',
  } as unknown as Parameters<typeof chatExchangePropsEqual>[0]);

  const turn = (liveReplyText: string): Exchange => ({
    userEvent: {
      type: 'MessageReceived',
      text: 'what is waiting',
      _eventId: 'm1',
      channel: 'chat',
    } as StoredEvent,
    userSeq: 1,
    steps: [{
      seq: Number.MAX_SAFE_INTEGER,
      event: {
        type: 'SpokenReplyGenerated',
        session_id: '',
        text: liveReplyText,
        interrupted: false,
        _eventId: 'live-reply:t:1',
        _liveReply: true,
      } as StoredEvent,
    }],
    liveReplyText,
  });

  it('re-renders on every word', () => {
    const before = props(turn("You're looking"));
    const after = props(turn("You're looking to send your answer back to that thread?"));
    expect(chatExchangePropsEqual(before, after)).toBe(false);
  });

  it('still swallows a recompute that changed nothing', () => {
    const same = () => props(turn("You're looking"));
    expect(chatExchangePropsEqual(same(), same())).toBe(true);
  });
});

describe('the mark', () => {
  it('holds still for a reader who asked for no motion', () => {
    expect(voiceCallCss).toMatch(
      /:root\[data-motion="reduce"\] \.live-speech-bar\s*\{[^}]*animation: none/,
    );
  });

  it('runs on a literal duration, outside the animation-speed scale', () => {
    expect(voiceCallCss).toContain('animation: live-speech-pulse 1s ease-in-out infinite');
    expect(voiceCallCss).not.toContain('live-speech-pulse var(--duration');
  });

  const body = (exchange: Exchange, html = '') =>
    vnodeToText(describeInitiator(exchange, html, [], 'tid').details);

  it('is the one mark, before their first word and after it', () => {
    const partial: Exchange = {
      ...theRow,
      userEvent: { ...theRow.userEvent, text: 'fix the bl', _livePartial: true } as StoredEvent,
    };
    expect(body(theRow)).toContain('live-speech-mark');
    expect(body(partial)).toContain('live-speech-mark');
  });

  it('is never drawn on the talker\'s reply', () => {
    const reply: Exchange = {
      userEvent: {
        type: 'SpokenReplyGenerated',
        session_id: '',
        text: 'On it',
        interrupted: false,
        _eventId: 'live-reply:t:1',
        _liveReply: true,
      } as StoredEvent,
      userSeq: Number.MAX_SAFE_INTEGER,
      steps: [],
    };
    expect(body(reply)).toContain('On it');
    expect(body(reply)).not.toContain('live-speech-mark');
  });

  it('leaves no caret behind', () => {
    expect(voiceCallCss).not.toContain('live-caret');
    expect(partsSource).not.toContain('live-caret');
  });
});
