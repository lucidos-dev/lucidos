// @vitest-environment jsdom
/** A side question asked during a turn sits at its moment inside that turn:
 *  rows written before it draw above the card, rows written after it below.
 *  See docs/plans/2026-09-28-side-question-sits-inside-its-turn.md. */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange, chatExchangePropsEqual } from '../ChatExchange';
import { placeInBody, sideQuestionOwners } from '../SideQuestionCard';
import { collapsedExchanges, detailsExpanded, stepsExpanded } from '../../../store/store';
import type { BodySection } from '../../../store/event-rendering';
import type { SideQuestion } from '../../../store/sideQuestions';
import type { Exchange, StoredEvent } from '../../../store/thread-events';
import type { ResponseEvent } from '../../../store/types';

const card = (id: string, afterSeq: number | null): SideQuestion =>
  ({ id, threadId: 't1', question: id, afterSeq, dismissed: false, status: 'pending' });

describe('placeInBody', () => {
  const step = (index: number, seq?: number) => ({
    event: { type: 'step', description: `s${index}`, outcome: 'success', seq } as Extract<ResponseEvent, { type: 'step' }>,
    index,
  });
  const text = (key: number, seq: number) => ({
    kind: 'text' as const, key, open: true, event: { type: 'text' as const, md: `t${key}`, seq },
  });
  const sections: BodySection[] = [{
    key: 0,
    rows: [
      text(0, 3),
      { kind: 'steps', key: 1, open: true, elided: false, steps: [step(1, 5), step(2, 9), step(3)] },
    ],
  }];
  const shape = (pieces: ReturnType<typeof placeInBody>) => pieces.map((p) => (p.kind === 'cards'
    ? `cards ${p.key} [${p.items.map((i) => i.id).join(',')}]`
    : `section ${p.key}: ${p.rows.map((r) => (r.kind === 'steps' ? r.steps.map((s) => `s${s.index}`).join('+') : `t${r.key}`)).join(' ')}`));

  it('splits a step run at the first step later than the question', () => {
    expect(shape(placeInBody(sections, [card('q', 6)]))).toEqual([
      'section 0: t0 s1',
      'cards side-questions:after:1 [q]',
      'section 2: s2+s3',
    ]);
  });

  it('puts the live row, which has no seq, below the question', () => {
    expect(shape(placeInBody(sections, [card('q', 9)]))).toEqual([
      'section 0: t0 s1+s2',
      'cards side-questions:after:2 [q]',
      'section 3: s3',
    ]);
  });

  it('draws a question older than every row first', () => {
    expect(shape(placeInBody(sections, [card('q', 1)]))).toEqual([
      'cards side-questions:after:start [q]',
      'section 0: t0 s1+s2+s3',
    ]);
  });

  it('keeps the box key as rows land after it', () => {
    const settled: BodySection[] = [{ key: 0, rows: [text(0, 3)] }];
    const grown: BodySection[] = [{ key: 0, rows: [text(0, 3), text(4, 12)] }];
    const keyOf = (s: BodySection[]) => placeInBody(s, [card('q', 6)]).find((p) => p.kind === 'cards')!.key;
    expect(keyOf(grown)).toBe(keyOf(settled));
  });

  it('changes nothing for a turn without questions', () => {
    expect(shape(placeInBody(sections, []))).toEqual(['section 0: t0 s1+s2+s3']);
  });

  describe('a text chunk streamed across the question', () => {
    const chunkOf = (md: string, pieces: { at: number; seq: number }[]): BodySection[] => [{
      key: 0,
      rows: [{ kind: 'text', key: 0, open: true, event: { type: 'text', md, seq: pieces[0].seq, pieces } }],
    }];
    const texts = (pieces: ReturnType<typeof placeInBody>) => pieces.map((p) => (p.kind === 'cards'
      ? 'card'
      : p.rows.map((r) => (r.kind === 'text' ? `${r.key}=${JSON.stringify(r.event.md)}` : '')).join(' ')));

    it('splits at the first paragraph break after the question', () => {
      const md = 'One.\n\nTwo.\n\nThree.';
      const placed = placeInBody(chunkOf(md, [{ at: 0, seq: 3 }, { at: 6, seq: 7 }, { at: 12, seq: 9 }]), [card('q', 5)]);
      expect(texts(placed)).toEqual(['0="One.\\n\\n"', 'card', '0@6="Two.\\n\\nThree."']);
    });

    it('waits for a paragraph break before it splits', () => {
      const md = 'One and more words';
      const placed = placeInBody(chunkOf(md, [{ at: 0, seq: 3 }, { at: 8, seq: 7 }]), [card('q', 5)]);
      expect(texts(placed)).toEqual(['0="One and more words"', 'card']);
    });

    it('never splits inside a code fence', () => {
      const md = 'Intro\n\n```\na\n\nb\n```\n\nAfter.';
      const placed = placeInBody(chunkOf(md, [{ at: 0, seq: 3 }, { at: 12, seq: 7 }]), [card('q', 5)]);
      expect(texts(placed)).toEqual(['0="Intro\\n\\n```\\na\\n\\nb\\n```\\n\\n"', 'card', '0@21="After."']);
    });

    it('keeps the tail key as the chunk grows', () => {
      const first = placeInBody(chunkOf('One.\n\nTwo', [{ at: 0, seq: 3 }, { at: 6, seq: 7 }]), [card('q', 5)]);
      const grown = placeInBody(chunkOf('One.\n\nTwo more.', [{ at: 0, seq: 3 }, { at: 6, seq: 7 }, { at: 9, seq: 8 }]), [card('q', 5)]);
      const tailKey = (p: ReturnType<typeof placeInBody>) => texts(p)[2].split('=')[0];
      expect(tailKey(grown)).toBe(tailKey(first));
    });
  });
});

describe('sideQuestionOwners', () => {
  const ex = (userSeq: number): Exchange => ({ userEvent: { type: 'MessageReceived', text: '' } as StoredEvent, userSeq, steps: [] });
  const exchanges = [ex(1), ex(10), ex(20)];

  it('gives each card to the latest turn begun before it', () => {
    const { owned, unowned } = sideQuestionOwners(exchanges, [card('a', 15), card('b', 25)], () => false);
    expect(owned.get(1)?.map((c) => c.id)).toEqual(['a']);
    expect(owned.get(2)?.map((c) => c.id)).toEqual(['b']);
    expect(unowned).toEqual([]);
  });

  it('never gives a card to a turn that draws no panel of its own', () => {
    const { owned } = sideQuestionOwners(exchanges, [card('a', 25)], (i) => i === 2);
    expect(owned.get(1)?.map((c) => c.id)).toEqual(['a']);
  });

  it('leaves a card with no owning turn to the feed', () => {
    const { owned, unowned } = sideQuestionOwners(exchanges, [card('a', 0), card('b', null)], () => false);
    expect(owned.size).toBe(0);
    expect(unowned.map((c) => c.id)).toEqual(['a', 'b']);
  });
});

describe('a running turn with a side question', () => {
  const at = (seq: number, event: Record<string, unknown>) => ({
    seq,
    event: { created: '2026-01-01T12:00:00Z', _eventId: `e${seq}`, ...event } as StoredEvent,
  });
  const running: Exchange = {
    userEvent: { type: 'MessageReceived', text: 'go', created: '2026-01-01T12:00:00Z', _eventId: 'm1' } as StoredEvent,
    userSeq: 1,
    steps: [
      at(2, { type: 'TextStreamed', text: 'Before the question.' }),
      at(3, { type: 'ToolCalled', name: 'search', args: {} }),
      at(4, { type: 'ToolResult', name: 'search', result: 'ok' }),
      at(5, { type: 'TextStreamed', text: 'After the question.' }),
    ],
  };
  let host: HTMLDivElement;

  const show = (exchange: Exchange, sideQuestions: SideQuestion[]) => act(() => {
    render(
      <ChatExchange
        exchange={exchange}
        revision={0}
        streamingBuffer=""
        isLast={true}
        threadId="t1"
        threadIsCC={false}
        threadCodingAgent="claude-code"
        threadIdle={false}
        threadAwaitingAnswer={false}
        threadCanceling={false}
        sideQuestions={sideQuestions}
      />,
      host,
    );
  });
  const chunk = (words: string) =>
    Array.from(host.querySelectorAll('.response-chunk')).find((c) => c.textContent?.includes(words))!;
  const statusText = () => host.querySelector('.exchange-status-label')?.textContent ?? '';

  beforeEach(() => {
    stepsExpanded.value = true;
    detailsExpanded.value = true;
    host = document.createElement('div');
    document.body.appendChild(host);
  });
  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('draws earlier rows above the card and later rows below it', () => {
    show(running, [card('q', 4)]);
    const box = host.querySelector('[data-role="side-questions"]')!;
    expect(chunk('Before the question.').compareDocumentPosition(box) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(box.compareDocumentPosition(chunk('After the question.')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('draws prose streamed after the question below the card', () => {
    const streaming: Exchange = {
      ...running,
      steps: [
        at(2, { type: 'TextStreamed', text: 'Written before.\n\n' }),
        at(3, { type: 'TextStreamed', text: 'Written after.' }),
      ],
    };
    show(streaming, [card('q', 2)]);
    const box = host.querySelector('[data-role="side-questions"]')!;
    expect(chunk('Written before.').compareDocumentPosition(box) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(box.compareDocumentPosition(chunk('Written after.')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('never makes the card a row of a response section', () => {
    show(running, [card('q', 4)]);
    expect(host.querySelector('.response-content [data-role="side-questions"]')).toBeNull();
  });

  it('leaves the turn status as it was', () => {
    show(running, []);
    const without = statusText();
    show(running, [card('q', 4)]);
    expect(statusText()).toBe(without);
  });

  it('draws the card after the turn when the body has no rows yet', () => {
    show({ ...running, steps: [] }, [card('q', 1)]);
    expect(host.querySelector('[data-role="side-questions"]')).not.toBeNull();
  });

  it('still shows the card when hidden steps leave the turn no body', () => {
    stepsExpanded.value = false;
    show({ ...running, steps: running.steps.slice(1, 3) }, [card('q', 3)]);
    expect(host.querySelector('[data-role="side-questions"]')).not.toBeNull();
  });

  it('still shows the card on a folded turn', () => {
    collapsedExchanges.value = new Set(['t1:1']);
    try {
      show(running, [card('q', 4)]);
      expect(host.querySelector('.response-chunk')).toBeNull();
      expect(host.querySelector('[data-role="side-questions"]')).not.toBeNull();
    } finally {
      collapsedExchanges.value = new Set();
    }
  });

  it('re-renders a turn only when its own cards change', () => {
    const props = {
      exchange: running, revision: 0, streamingBuffer: '', isLast: true, threadId: 't1', threadIsCC: false,
      threadCodingAgent: 'claude-code' as const, threadIdle: false, threadAwaitingAnswer: false, threadCanceling: false,
    };
    const asked = card('q', 4);
    expect(chatExchangePropsEqual({ ...props, sideQuestions: [asked] }, { ...props, sideQuestions: [asked] })).toBe(true);
    expect(chatExchangePropsEqual({ ...props }, { ...props, sideQuestions: [] })).toBe(true);
    const answered: SideQuestion = { ...asked, status: 'answered', answer: 'a' };
    expect(chatExchangePropsEqual({ ...props, sideQuestions: [asked] }, { ...props, sideQuestions: [answered] })).toBe(false);
  });
});
