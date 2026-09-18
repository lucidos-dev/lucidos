/** A call with nothing behind it draws no turn header.
 *
 *  Two people talking is a turn nobody executed. A "Lucidos Agent" row between
 *  two speech bubbles names an actor the reader knows. It also dates a sentence
 *  they just heard, and offers three controls over a body of one line.
 *
 *  What keeps its header is the card the DOER is working under. That one holds
 *  a turn, and the header is what says so.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { ResponsePanel } from '../chat-exchange-parts';
import { vnodeToText } from './vnodeToText';
import { isSpeechOnlyTurn } from '../../../store/thread-events';
import type { Exchange, SequencedEvent, StoredEvent } from '../../../store/thread-events';

const here: string = dirname(fileURLToPath(import.meta.url));
const chatExchangeSource: string = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

function exchangeWith(userEvent: StoredEvent, steps: SequencedEvent[] = []): Exchange {
  return { userEvent, userSeq: 0, steps };
}

const step = (event: Partial<StoredEvent> & { type: string }, seq = 1): SequencedEvent =>
  ({ seq, event: event as StoredEvent });

const spokenReply = (text: string) =>
  step({ type: 'SpokenReplyGenerated', session_id: 'sess-1', text, interrupted: false });

/** The caller's own utterance, in the spelling every call writes today. */
const heard = (text: string) =>
  ({ type: 'SpokenMessageReceived', session_id: 'sess-1', text }) as unknown as StoredEvent;

describe('a card the talker answered by itself', () => {
  it('holds only speech', () => {
    expect(isSpeechOnlyTurn(exchangeWith(heard('what is up'), [spokenReply('Not much.')])))
      .toBe(true);
  });

  it('still holds only speech with the session marks around it', () => {
    const marks = [
      step({ type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      spokenReply('Not much.'),
      step({ type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup' }, 2),
    ];
    expect(isSpeechOnlyTurn(exchangeWith(heard('what is up'), marks))).toBe(true);
  });

  /** The pre-ADR-0201 spelling: a `MessageReceived` marked with the session. */
  it('reads a legacy row the same way', () => {
    const legacy = {
      type: 'MessageReceived',
      text: 'what is up',
      voice_session_id: 'sess-1',
    } as unknown as StoredEvent;
    expect(isSpeechOnlyTurn(exchangeWith(legacy, [spokenReply('Not much.')]))).toBe(true);
  });

  /** The talker spoke before anybody else did, so its greeting opened a
   *  boundary of its own and draws in the initiator body. */
  it('takes the greeting too', () => {
    const greeting = {
      type: 'SpokenReplyGenerated',
      session_id: 'sess-1',
      text: 'Hi there.',
      interrupted: false,
    } as unknown as StoredEvent;
    expect(isSpeechOnlyTurn(exchangeWith(greeting))).toBe(true);
  });

  /** No engine event backs the caller's pulse: it is a bare `MessageReceived`
   *  with no session on it, so the mark is the only thing that can answer. */
  it('takes a live row, which no session id marks', () => {
    const pulse = {
      type: 'MessageReceived',
      text: '',
      _liveUtterance: true,
    } as unknown as StoredEvent;
    expect(isSpeechOnlyTurn(exchangeWith(pulse))).toBe(true);
  });
});

describe('a card with work behind it keeps its header', () => {
  it('drops out the moment the doer does something', () => {
    const worked = [
      spokenReply('Let me look.'),
      step({ type: 'ToolCalled', name: 'read_file', args: {}, description: 'Reading…' }, 2),
    ];
    expect(isSpeechOnlyTurn(exchangeWith(heard('what is waiting'), worked))).toBe(false);
  });

  /** The delegation itself draws nothing, so the steps alone still read as
   *  speech. `tookTheTurn` is the fact that the doer was asked, and the reader
   *  is owed the Working badge on this card while it sleeps. */
  it('drops out on a delegation, before the doer has woken', () => {
    const delegated: Exchange = {
      ...exchangeWith(heard('send that email'), [
        spokenReply('On it.'),
        step({ type: 'WorkDelegated', session_id: 'sess-1' }, 2),
      ]),
      tookTheTurn: true,
    };
    expect(isSpeechOnlyTurn(delegated)).toBe(false);
  });

  it('says nothing about an ordinary typed turn', () => {
    const typed = { type: 'MessageReceived', text: 'fix the build' } as unknown as StoredEvent;
    expect(isSpeechOnlyTurn(exchangeWith(typed, [spokenReply('x')]))).toBe(false);
  });
});

describe('the panel that draws it', () => {
  const panel = (over: { headerless?: boolean; collapsed?: boolean } = {}) => ResponsePanel({
    executor: { icon: null, label: 'Lucidos Agent' },
    controls: null,
    status: null,
    timestamp: '15:06',
    collapsed: false,
    hasBody: true,
    children: 'Not much at the moment.',
    ...over,
  });

  it('draws the header by default', () => {
    expect(vnodeToText(panel())).toContain('class="response-header"');
    expect(vnodeToText(panel())).toContain('Lucidos Agent');
  });

  it('drops the header and keeps the words', () => {
    const drawn = vnodeToText(panel({ headerless: true }));
    expect(drawn).not.toContain('response-header');
    expect(drawn).not.toContain('Lucidos Agent');
    expect(drawn).toContain('class="response-body"');
    expect(drawn).toContain('Not much at the moment.');
  });

  /** The collapse control is in the row that went, so a fold here would have
   *  no way back out. A key left in the store by an earlier fold must not
   *  strand the words behind a stub nothing can clear. */
  it('never folds without a header, whatever the store remembers', () => {
    const drawn = vnodeToText(panel({ headerless: true, collapsed: true }));
    expect(drawn).toContain('Not much at the moment.');
    expect(drawn).not.toContain('turn-collapsed');
  });
});

describe('ChatExchange wires it in', () => {
  it('hands the decision to the panel', () => {
    expect(chatExchangeSource).toMatch(/<ResponsePanel[\s\S]*?headerless=\{isSpeechOnly\}/);
  });

  /** A status badge has only the header to sit on. So a card still waiting on
   *  its reply would draw an empty box under the bubble. */
  it('withholds the panel until there are words in it', () => {
    expect(chatExchangeSource).toMatch(/const speechOnlyHasWords = !isSpeechOnly \|\| canCollapse/);
    expect(chatExchangeSource).toMatch(/showResponsePanel\s*=[^;]*speechOnlyHasWords/);
  });

  /** Neither panel draws a `⋯` stub here, so ⌘↑/⌘↓ plus Enter must not offer
   *  a fold that nothing can undo. */
  it('offers the keyboard no fold on such a turn', () => {
    expect(chatExchangeSource).toMatch(/const collapseKind = isSpeechOnly \? undefined/);
  });
});
