/** What the caller's bubble holds before their words exist.
 *
 *  No engine event backs this row. So nothing downstream can be relied on to
 *  treat it sensibly by accident. Three promises: it is the ordinary
 *  right-aligned user bubble, it says something to a screen reader, and no
 *  panel is drawn under it.
 *
 *  The third is a source scan. The decision lives inside `ChatExchange`, and
 *  there is no jsdom here to mount it in, so the expression is what can be
 *  read. Same shape as `composer-live-during-a-call.test.ts`.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { chatExchangePropsEqual, describeInitiator, isUserBubbleEvent } from '../ChatExchange';
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
    expect(line).toContain('!isLiveUtterance');
  });

  /** And the exclusion lifts the moment the words are there. The engine is
   *  then holding them while the talker decides. Something IS in flight, and
   *  the reader is owed the shimmer under their own turn (ADR 0174). */
  it('stops being excluded once the row carries the words', () => {
    const line = chatExchangeSource
      .split('\n')
      .find(l => l.includes('const isLiveUtterance ='));
    expect(line).toContain('!userMessage');
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

  /** The waiting is real: the engine is holding the words. So the turn reads
   *  as pending, which is the Requesting shimmer, rather than as a finished
   *  turn with no answer. */
  it('reads as a turn still being requested', () => {
    expect(exchangeStatus(spoken, '', true, false, false, /* threadIdle */ true, false))
      .toBe('pending');
  });

  it('reads the same way while the caller is still speaking', () => {
    expect(exchangeStatus(theRow, '', true, false, false, /* threadIdle */ true, false))
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
