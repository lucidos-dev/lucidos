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
    expect(body).toContain('live-utterance-bar');
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

  it('draws the words, not the pulse', () => {
    const body = vnodeToText(describeInitiator(spoken, '<p>fix the blank thread</p>', [], 'tid').details);
    expect(body).toContain('fix the blank thread');
    expect(body).not.toContain('live-utterance-bar');
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

describe('the pulse', () => {
  it('holds still for a reader who asked for no motion', () => {
    const reduce = voiceCallCss.slice(voiceCallCss.indexOf('@media (prefers-reduced-motion'));
    expect(reduce).toContain('.live-utterance-bar');
    expect(reduce).toContain('animation: none');
  });

  /** An indefinite animation is an activity indicator rather than a
   *  transition, so it keeps a literal duration and never a `--duration-*`
   *  token. See `.claude/rules/frontend-css.md`. */
  it('runs on a literal duration, outside the animation-speed scale', () => {
    expect(voiceCallCss).toContain('animation: live-utterance-pulse 1s ease-in-out infinite');
    expect(voiceCallCss).not.toContain('live-utterance-pulse var(--duration');
  });
});
