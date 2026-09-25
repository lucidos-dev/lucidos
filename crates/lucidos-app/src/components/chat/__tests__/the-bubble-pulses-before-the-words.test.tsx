/** What the caller's bubble holds before their words exist.
 *
 *  No engine event backs this row. So nothing downstream can be relied on to
 *  treat it sensibly by accident. Three promises: it is the ordinary
 *  right-aligned user bubble, it says something to a screen reader, and no
 *  panel is drawn under it.
 *
 *  The third is read off `liveRowDrawsNoPanel`, the decision itself, rather
 *  than off the source line that consumes it. One source scan is left, and it
 *  only checks that `showResponsePanel` still consults that decision at all.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { chatExchangePropsEqual, describeInitiator, isUserBubbleEvent, liveRowDrawsNoPanel } from '../ChatExchange';
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

/** The row exactly as `computeExchanges` appends it: a `MessageReceived` with
 *  no text, marked as the one nothing wrote. */
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

  /** An animation says nothing to a screen reader, and a bubble with no name
   *  reads as empty. The phrase is the one the call toggle's status region
   *  speaks, so the two cannot drift apart. */
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

  /** And the exclusion lifts the moment the FINAL words are there. The engine
   *  is then holding them while the talker decides. Something IS in flight,
   *  and the reader is owed the shimmer under their own turn (ADR 0174).
   *
   *  A partial does not lift it. The caller is still speaking, so nothing is
   *  in flight behind the bubble yet.
   *
   *  Read off the RENDERED decision rather than off the source line. A token
   *  scan passes on any code containing the token and fails on a reformat, so
   *  it measures the spelling instead of the behaviour. */
  it('draws a panel for the final words and none for a partial', () => {
    expect(liveRowDrawsNoPanel({ ...theRow.userEvent, text: 'the tests' } as StoredEvent)).toBe(false);
    expect(liveRowDrawsNoPanel({
      ...theRow.userEvent,
      text: 'the tes',
      _livePartial: true,
    } as StoredEvent)).toBe(true);
    expect(liveRowDrawsNoPanel(theRow.userEvent)).toBe(true);
  });

  /** The talker's own live row takes the same exclusion. The words moving in
   *  it are the activity, so a status badge below would be a second one. */
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

/** The same row, once the provider has ended the turn. It is the caller's own
 *  message now, and takes the bubble a spoken message takes. */
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

  /** Nothing is in flight behind a sentence the caller has finished. The
   *  engine holds it until the conversation moves, which is a whole reply
   *  long. A Requesting shimmer over it says something untrue for all of that.
   *  Whatever the words do start arrives as its own exchange. */
  it('reads as settled once the caller has finished saying it', () => {
    expect(exchangeStatus(spoken, '', true, false, false, /* threadIdle */ true, false))
      .toBe('done');
  });

  /** The pulse is the other half, and it still shimmers: they are mid-word,
   *  so the reader really is waiting on the rest of the sentence. */
  it('still reads as pending while the caller is speaking', () => {
    expect(exchangeStatus(theRow, '', true, false, false, /* threadIdle */ true, false))
      .toBe('pending');
  });

  /** A partial is words the provider may still revise, so it is the pulse's
   *  half rather than the finished sentence's. */
  it('still reads as pending on a partial', () => {
    const partial: Exchange = {
      ...theRow,
      userEvent: { ...theRow.userEvent, text: 'fix the blank', _livePartial: true } as StoredEvent,
    };
    expect(exchangeStatus(partial, '', true, false, false, /* threadIdle */ true, false))
      .toBe('pending');
  });

  /** The row is `memo`d on a content fingerprint, and its identity does not
   *  move when the words arrive: same event id, same seq, no steps, no
   *  revision. So the fingerprint has to carry the text, or the swap is
   *  swallowed and the pulse keeps drawing over words the reader could read.
   *  That is the whole reported bug, arriving one layer down. */
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

/** The reported freeze. The talker's live row is a STEP, so the fingerprint's
 *  step count and its last seq both hold across every word the row gains. The
 *  bubble stopped at `You're looking` while the mark kept moving beside it. */
describe('the talker\'s bubble while it is being said', () => {
  const props = (exchange: Exchange) => ({
    exchange,
    revision: 0,
    threadId: 'tid',
    isLast: false,
    streamingBuffer: '',
  } as unknown as Parameters<typeof chatExchangePropsEqual>[0]);

  /** One doer turn holding the talker's live row, exactly as
   *  `withLiveCallRows` builds it. */
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

  /** And the memo still earns its keep: a recompute that changed nothing must
   *  not re-render every sibling on every event. */
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

  /** An indefinite animation is an activity indicator rather than a
   *  transition, so it keeps a literal duration and never a `--duration-*`
   *  token. See `.claude/rules/frontend-css.md`. */
  it('runs on a literal duration, outside the animation-speed scale', () => {
    expect(voiceCallCss).toContain('animation: live-speech-pulse 1s ease-in-out infinite');
    expect(voiceCallCss).not.toContain('live-speech-pulse var(--duration');
  });

  const body = (exchange: Exchange, html = '') =>
    vnodeToText(describeInitiator(exchange, html, [], 'tid').details);

  /** The caller's bubble means one thing by it through both of its shapes: the
   *  words are still arriving, and the provider has not settled them. Two
   *  shapes for one meaning is what the caret was. */
  it('is the one mark, before their first word and after it', () => {
    const partial: Exchange = {
      ...theRow,
      userEvent: { ...theRow.userEvent, text: 'fix the bl', _livePartial: true } as StoredEvent,
    };
    expect(body(theRow)).toContain('live-speech-mark');
    expect(body(partial)).toContain('live-speech-mark');
  });

  /** **The talker's row draws no mark, live or landed** (ADR 0197). Its live
   *  row is retired by the engine's own, written at the next move of the
   *  conversation (ADR 0188, ADR 0191). So the mark would stand over a
   *  finished sentence until the caller spoke again, which is what was
   *  reported. Gating it on the call phase blinks it instead. */
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

  /** The caret it replaced is gone from both layers, so nothing can draw the
   *  old shape by reaching for a class that still styles. */
  it('leaves no caret behind', () => {
    expect(voiceCallCss).not.toContain('live-caret');
    expect(partsSource).not.toContain('live-caret');
  });
});
