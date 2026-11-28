import { describe, expect, it } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { SpokenChip, SpokenReply } from '../chat-exchange-parts';
import { describeInitiator } from '../ChatExchange';
import { isUserBubbleEvent } from '../../../store/thread-events/exchange';
import { vnodeToText } from './vnodeToText';
import { drawsResponseRow, getCollapsedVisibleEvents } from '../../../store/event-rendering';
import { exchangeResponseEvents } from '../../../store/thread-events';
import type { Exchange } from '../../../store/thread-events';
import type { ResponseEvent } from '../../../store/types';

interface AnyVNode extends VNode<{ children?: ComponentChildren; [k: string]: unknown }> {}

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

function hasHiddenName(node: ComponentChildren, text: string): boolean {
  return vnodeToText(node).includes(`<span class="visually-hidden">${text}</span>`);
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
    expect(typed.status).toBeUndefined();
  });

  it('takes no surface of its own', () => {
    const spoken = describeInitiator(
      exchangeWith({
        type: 'MessageReceived',
        text: 'is the deploy finished',
        mode: 'human',
        voice_session_id: 'sess-1',
      }),
      '<p>is the deploy finished</p>',
      [],
      'tid',
    );
    expect(spoken.accent).toBeUndefined();
  });

  it('is the call icon alone, and still says "spoken" to a screen reader', () => {
    const chip = SpokenChip();
    expect(findByRole(chip, 'spoken-chip')).not.toBeNull();
    expect(vnodeText(chip)).toBe('Spoken');
    expect(hasHiddenName(chip, 'Spoken')).toBe(true);
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
      seq: 1,
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
    expect(drawsResponseRow(spokenRow(), false)).toBe(true);
  });

  it('survives the collapse that drops mechanics and earlier prose', () => {
    const events: ResponseEvent[] = [
      spokenRow({ text: 'Let me check that.' }),
      { type: 'step', description: 'Ran a query', outcome: 'success' },
      { type: 'text', md: 'Both endpoints answered live.' },
    ];
    const visibleEvents = getCollapsedVisibleEvents(events);
    expect(visibleEvents).toContainEqual(spokenRow({ text: 'Let me check that.' }));
  });
});

describe('a spoken reply is not the written answer', () => {
  it('wears its own mark, outside the response prose', () => {
    const row = SpokenReply({ event: spokenRow({ text: 'Both of them answered.' }) });
    expect(findByRole(row, 'spoken-reply')).not.toBeNull();
    expect(vnodeText(row)).toContain('Both of them answered.');
    expect(hasHiddenName(row, 'Said aloud')).toBe(true);
  });

  it('says so when the caller talked over it', () => {
    const cut = SpokenReply({ event: spokenRow({ interrupted: true }) });
    expect(vnodeText(cut)).toContain('cut off');
    const whole = SpokenReply({ event: spokenRow({ interrupted: false }) });
    expect(vnodeText(whole)).not.toContain('cut off');
  });

  it('marks the first of a run and no other', () => {
    const reply = (text: string) => ({
      seq: 1,
      event: { type: 'SpokenReplyGenerated', session_id: 'sess-1', text, interrupted: false },
    });
    const events = exchangeResponseEvents(
      exchangeWith({ type: 'MessageReceived', text: 'hi', mode: 'human' }, [
        reply('Let me check.'),
        reply('One moment.'),
        { seq: 2, event: { type: 'ToolCalled', name: 'read_file', args: {}, description: 'Reading…' } },
        reply('Here it is.'),
      ] as Exchange['steps']),
    );
    const spoken = events.filter(e => e.type === 'spoken_reply');
    expect(spoken.map(e => e.follows === true)).toEqual([false, true, false]);

    const led = SpokenReply({ event: spokenRow() });
    const inRun = SpokenReply({ event: { ...spokenRow(), follows: true } });
    expect(hasHiddenName(led, 'Said aloud')).toBe(true);
    expect(hasHiddenName(inRun, 'Said aloud')).toBe(false);
    expect(findByRole(inRun, 'spoken-reply')).not.toBeNull();
  });

  it('never says it is still being said', () => {
    const drawn = vnodeToText(SpokenReply({ event: spokenRow() }));
    expect(drawn).not.toContain('live-speech-mark');
    expect(drawn).not.toContain('Saying aloud');
    expect(hasHiddenName(SpokenReply({ event: spokenRow() }), 'Said aloud')).toBe(true);
  });
});

describe('a caller utterance reads the same either way', () => {
  const HTML = '<p>what happened?</p>';

  const delegated = () => describeInitiator(
    exchangeWith({
      type: 'MessageReceived',
      text: 'what happened?',
      mode: 'human',
      channel: 'chat',
      voice_session_id: 'sess-1',
    }),
    HTML,
    [],
    'tid',
  );

  const fieldedAlone = () => describeInitiator(
    exchangeWith({ type: 'SpokenMessageReceived', session_id: 'sess-1', text: 'what happened?' }),
    HTML,
    [],
    'tid',
  );

  it('is the same panel, whoever answered it', () => {
    const a = delegated();
    const b = fieldedAlone();
    expect(b.variant).toBe(a.variant);
    expect(b.label).toBe(a.label);
    expect(b.accent).toBe(a.accent);
    expect(b.summary).toBe(a.summary);
    expect(vnodeToText(b.details)).toBe(vnodeToText(a.details));
    expect(vnodeText(b.status)).toBe(vnodeText(a.status));
  });

  it('draws the right-aligned user bubble on both', () => {
    expect(isUserBubbleEvent({ type: 'MessageReceived' })).toBe(true);
    expect(isUserBubbleEvent({ type: 'SpokenMessageReceived' })).toBe(true);
    expect(delegated().variant).toBe('user');
    expect(fieldedAlone().variant).toBe('user');
  });

  it('carries the words, and never an inline row of its own', () => {
    expect(vnodeToText(fieldedAlone().details)).toContain('what happened?');
    const events = exchangeResponseEvents(
      exchangeWith({ type: 'MessageReceived', text: 'hi', mode: 'human' }, [
        {
          seq: 1,
          event: { type: 'SpokenMessageReceived', session_id: 'sess-1', text: 'What happened?' },
        },
      ]),
    );
    expect(events).toEqual([]);
  });
});

describe('a greeting said before any turn existed still draws', () => {
  it('carries the words in its own panel', () => {
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
});
