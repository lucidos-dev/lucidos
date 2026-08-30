/** A call leaves two marks in the transcript, and they say different things.
 *
 *  The bubble mark says the message was SPOKEN. The session bounds cannot say
 *  it: the composer stays live during a call (ADR 0148), so a typed message
 *  sits between the same pair of session events.
 *
 *  The row says what Lucidos said back OUT LOUD, which is never the written
 *  answer beside it: the talker says what an answer means. It is a transcript
 *  marker rather than step mechanics, so no control may hide it. No audio is
 *  kept, so the row is the only record that exists.
 */
import { describe, expect, it } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { SpokenMessage, SpokenReply } from '../chat-exchange-parts';
import { describeInitiator } from '../ChatExchange';
import { drawsResponseRow, getCollapsedVisibleEvents } from '../../../store/event-rendering';
import { exchangeResponseEvents } from '../../../store/thread-events';
import type { Exchange } from '../../../store/thread-events';
import type { ResponseEvent } from '../../../store/types';

interface AnyVNode extends VNode<{ children?: ComponentChildren; [k: string]: unknown }> {}

/** The text a node would show. There is no jsdom here, so a child that is a
 *  function component is invoked rather than mounted. Every component this
 *  file reaches is hookless, which is what makes that safe. */
function vnodeText(n: ComponentChildren): string {
  if (n === null || n === undefined || typeof n === 'boolean') return '';
  if (typeof n === 'string' || typeof n === 'number') return String(n);
  if (Array.isArray(n)) return n.map(vnodeText).join('');
  const v = n as AnyVNode;
  if (typeof v.type === 'function') {
    return vnodeText((v.type as (props: unknown) => ComponentChildren)(v.props));
  }
  return vnodeText(v.props?.children);
}

function findByRole(node: ComponentChildren, role: string): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = findByRole(c, role);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  if (v.props?.['data-role'] === role) return v;
  return findByRole(v.props?.children, role);
}

const spokenRow = (over: Partial<Extract<ResponseEvent, { type: 'spoken_reply' }>> = {}) =>
  ({ type: 'spoken_reply', text: 'Both of them answered.', interrupted: false, ...over }) as const;

function exchangeWith(userEvent: Exchange['userEvent'], steps: Exchange['steps'] = []): Exchange {
  return { userEvent, userSeq: 0, steps };
}

describe('a spoken message says so', () => {
  it('marks the bubble, and a typed message carries no mark', () => {
    const spoken = describeInitiator(
      exchangeWith({
        type: 'MessageReceived',
        text: 'is the deploy finished',
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
      }),
      '<p>is the deploy finished</p>',
      [],
      'tid',
    );
    expect(spoken.label).toBe('You');
    expect(spoken.accent).toBe('spoken');
    expect(vnodeText(spoken.status)).toContain('Spoken');

    const typed = describeInitiator(
      exchangeWith({
        type: 'MessageReceived',
        text: 'is the deploy finished',
        mode: 'human',
        channel: 'chat',
      }),
      '<p>is the deploy finished</p>',
      [],
      'tid',
    );
    expect(typed.accent).toBeUndefined();
    expect(typed.status).toBeUndefined();
  });
});

describe('what was said out loud is in the transcript', () => {
  it('a spoken reply becomes a row of its own', () => {
    const events = exchangeResponseEvents(
      exchangeWith({ type: 'MessageReceived', text: 'hi', mode: 'human' }, [
        {
          seq: 1,
          event: {
            type: 'SpokenReplyGenerated',
            session_id: 'sess-1',
            text: 'Both of them answered.',
            interrupted: false,
          },
        },
      ]),
    );
    expect(events).toContainEqual({
      type: 'spoken_reply',
      text: 'Both of them answered.',
      interrupted: false,
    });
  });

  it('a reply with no words is not drawn', () => {
    const events = exchangeResponseEvents(
      exchangeWith({ type: 'MessageReceived', text: 'hi', mode: 'human' }, [
        {
          seq: 1,
          event: {
            type: 'SpokenReplyGenerated',
            session_id: 'sess-1',
            text: '   ',
            interrupted: true,
          },
        },
      ]),
    );
    expect(events.some((e) => e.type === 'spoken_reply')).toBe(false);
  });

  it('draws with the steps control OFF', () => {
    // The caller heard it and no audio is kept, so a default-off control would
    // record it nowhere. That is the bug the event row already taught us.
    expect(drawsResponseRow(spokenRow(), false)).toBe(true);
  });

  it('survives the collapse that drops mechanics and earlier prose', () => {
    const events: ResponseEvent[] = [
      spokenRow({ text: 'Let me check that.' }),
      { type: 'step', description: 'Ran a query', outcome: 'success' },
      { type: 'text', md: 'Both endpoints answered live.' },
    ];
    const { visibleEvents } = getCollapsedVisibleEvents(events);
    expect(visibleEvents).toContainEqual(spokenRow({ text: 'Let me check that.' }));
  });
});

describe('a spoken reply is not the written answer', () => {
  it('wears its own label, outside the response prose', () => {
    const row = SpokenReply({ event: spokenRow({ text: 'Both of them answered.' }) });
    expect(findByRole(row, 'spoken-reply')).not.toBeNull();
    expect(vnodeText(row)).toContain('Said aloud');
    expect(vnodeText(row)).toContain('Both of them answered.');
  });

  it('says so when the caller talked over it', () => {
    const cut = SpokenReply({ event: spokenRow({ interrupted: true }) });
    expect(vnodeText(cut)).toContain('cut off');
    const whole = SpokenReply({ event: spokenRow({ interrupted: false }) });
    expect(vnodeText(whole)).not.toContain('cut off');
  });
});

describe('what the caller said out loud is in the transcript too', () => {
  it('a spoken message the talker fielded alone becomes a row of its own', () => {
    // It started no turn, so unlike a delegated utterance it is no
    // `MessageReceived` and has no bubble anywhere else.
    const events = exchangeResponseEvents(
      exchangeWith({ type: 'MessageReceived', text: 'hi', mode: 'human' }, [
        {
          seq: 1,
          event: { type: 'SpokenMessageReceived', session_id: 'sess-1', text: 'What happened?' },
        },
      ]),
    );
    expect(events).toContainEqual({ type: 'spoken_message', text: 'What happened?' });
  });

  it('wears the caller label, and draws with the steps control OFF', () => {
    const row = SpokenMessage({ event: { type: 'spoken_message', text: 'What happened?' } });
    expect(findByRole(row, 'spoken-message')).not.toBeNull();
    expect(vnodeText(row)).toContain('You said');
    expect(vnodeText(row)).toContain('What happened?');
    expect(drawsResponseRow({ type: 'spoken_message', text: 'What happened?' }, false)).toBe(true);
  });
});

describe('a spoken turn said before any turn existed still draws', () => {
  it('a greeting boundary carries the words in its own panel', () => {
    const panel = describeInitiator(
      exchangeWith({
        type: 'SpokenReplyGenerated',
        session_id: 'sess-1',
        text: 'Hi there. How can I help?',
        interrupted: false,
      }),
      '',
      [],
      'tid',
    );
    expect(vnodeText(panel.details)).toContain('Hi there. How can I help?');
  });

  it('a caller utterance boundary reads as theirs, spoken', () => {
    const panel = describeInitiator(
      exchangeWith({ type: 'SpokenMessageReceived', session_id: 'sess-1', text: 'Are you there?' }),
      '',
      [],
      'tid',
    );
    expect(panel.label).toBe('You');
    expect(panel.accent).toBe('spoken');
    expect(vnodeText(panel.details)).toContain('Are you there?');
  });
});
