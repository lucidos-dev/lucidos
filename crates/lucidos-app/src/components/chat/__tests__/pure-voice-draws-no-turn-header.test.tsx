import { describe, expect, it, vi } from 'vitest';
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

vi.mock('../../shared/Disclosure', () => import('../../shared/__tests__/disclosureStub'));

const here: string = dirname(fileURLToPath(import.meta.url));
const chatExchangeSource: string = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

function exchangeWith(userEvent: StoredEvent, steps: SequencedEvent[] = []): Exchange {
  return { userEvent, userSeq: 0, steps };
}

const step = (event: Partial<StoredEvent> & { type: string }, seq = 1): SequencedEvent =>
  ({ seq, event: event as StoredEvent });

const spokenReply = (text: string) =>
  step({ type: 'SpokenReplyGenerated', session_id: 'sess-1', text, interrupted: false });

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

  it('reads a legacy row the same way', () => {
    const legacy = {
      type: 'MessageReceived',
      text: 'what is up',
      voice_session_id: 'sess-1',
    } as unknown as StoredEvent;
    expect(isSpeechOnlyTurn(exchangeWith(legacy, [spokenReply('Not much.')]))).toBe(true);
  });

  it('takes the greeting too', () => {
    const greeting = {
      type: 'SpokenReplyGenerated',
      session_id: 'sess-1',
      text: 'Hi there.',
      interrupted: false,
    } as unknown as StoredEvent;
    expect(isSpeechOnlyTurn(exchangeWith(greeting))).toBe(true);
  });

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

  it('never folds without a header, whatever the store remembers', () => {
    const drawn = vnodeToText(panel({ headerless: true, collapsed: true }));
    expect(drawn).toContain('Not much at the moment.');
  });
});

describe('ChatExchange wires it in', () => {
  it('hides the panel header for speech-only turns and sign-in panels', () => {
    expect(chatExchangeSource).toMatch(/<ResponsePanel[\s\S]*?headerless=\{isSpeechOnly \|\| isSignInPanel\}/);
  });

  it('withholds the panel until there are words in it', () => {
    expect(chatExchangeSource).toMatch(/const speechOnlyHasWords = !isSpeechOnly \|\| canCollapse/);
    expect(chatExchangeSource).toMatch(/showResponsePanel\s*=[^;]*speechOnlyHasWords/);
  });

  it('offers the keyboard no fold on such a turn', () => {
    expect(chatExchangeSource).toMatch(/const collapseKind = isSpeechOnly \? undefined/);
  });
});
