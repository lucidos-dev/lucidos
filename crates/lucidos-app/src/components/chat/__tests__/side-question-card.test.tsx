// @vitest-environment jsdom
/** The `/btw` side-question card (ADR 0320): the question, then a delayed
 *  "Thinking", the answer as markdown, or the error. It says the answer is not
 *  part of the conversation. Dismissed, it folds to a row that reopens. It
 *  stays where it was asked, and later turns draw below it. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, type VNode } from 'preact';
import { act } from 'preact/test-utils';
vi.mock('../../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../../api/client')>();
  return { ...actual, dismissSideQuestion: () => Promise.resolve() };
});

import { SIDE_QUESTION_NOTE, SIDE_QUESTION_THINKING_DELAY_MS, placeSideQuestions } from '../SideQuestionCard';
import {
  dismissingSideQuestions,
  localSideQuestions,
  reopenedSideQuestions,
  sideQuestionsFor,
  type SideQuestion,
} from '../../../store/sideQuestions';
import type { Exchange } from '../../../store/thread-events/exchange';
import type { StoredEvent } from '../../../store/thread-events/thread-event-types';

let host: HTMLDivElement;

/** The feed as ThreadView draws it, for a thread with no turns yet. */
function Feed() {
  return <>{placeSideQuestions([], [], sideQuestionsFor('t1'), false)}</>;
}

function show(items: SideQuestion[]) {
  act(() => {
    localSideQuestions.value = new Map(items.map((item) => [item.id, item]));
    render(<Feed />, host);
  });
}

const asked = { threadId: 't1', afterSeq: 0, dismissed: false };

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  localSideQuestions.value = new Map();
  dismissingSideQuestions.value = new Set();
  reopenedSideQuestions.value = new Set();
  vi.useRealTimers();
});

it('renders nothing for a thread without side questions', () => {
  show([{ ...asked, id: 'a', threadId: 'other', question: 'q', status: 'pending' }]);
  expect(host.innerHTML).toBe('');
});

it('renders the answer as markdown, with the not-recorded note', () => {
  show([{ ...asked, id: 'a', question: 'what is X?', status: 'answered', answer: 'X is **bold**.' }]);
  const card = host.querySelector('[data-role="side-question-card"]')!;
  expect(card.querySelector('.side-question-question')!.textContent).toBe('what is X?');
  expect(card.querySelector('.markdown-content strong')!.textContent).toBe('bold');
  expect(card.textContent).toContain(SIDE_QUESTION_NOTE);
});

it('says Thinking only after a delay while pending, shimmering like the live step row', () => {
  vi.useFakeTimers();
  show([{ ...asked, id: 'a', question: 'q', status: 'pending' }]);
  const thinking = host.querySelector('.side-question-thinking')!;
  expect(thinking.textContent).toBe('');
  act(() => { vi.advanceTimersByTime(SIDE_QUESTION_THINKING_DELAY_MS); });
  const label = host.querySelector('.side-question-thinking .running-shimmer')!;
  expect(label.textContent).toBe('Thinking');
});

it('shows a failure as an alert', () => {
  show([{ ...asked, id: 'a', question: 'q', status: 'failed', error: 'Side questions are not available in Codex threads.' }]);
  const alert = host.querySelector('[role="alert"]')!;
  expect(alert.textContent).toBe('Side questions are not available in Codex threads.');
});

it('collapses one card from a stack with a labelled chevron', () => {
  show([
    { ...asked, id: 'a', question: 'first', status: 'answered', answer: '1' },
    { ...asked, id: 'b', question: 'second', status: 'answered', answer: '2' },
  ]);
  const buttons = host.querySelectorAll<HTMLButtonElement>('button[aria-label="Collapse side question"]');
  expect(buttons).toHaveLength(2);
  expect(buttons[0].getAttribute('data-tooltip')).toBe('Collapse side question');
  act(() => { buttons[0].click(); });
  const questions = [...host.querySelectorAll('.side-question-question')].map((n) => n.textContent);
  expect(questions).toEqual(['second']);
  expect(host.querySelector('.side-question-dismissed-text')!.textContent).toBe('first');
});

it('folds a collapsed card to one line that expands it again', () => {
  show([{ ...asked, id: 'a', question: 'what is X?', status: 'answered', answer: 'X', dismissed: true }]);
  expect(host.querySelector('[data-role="side-question-card"]')).toBeNull();
  const row = host.querySelector<HTMLButtonElement>('[data-role="side-question-dismissed"]')!;
  expect(row.getAttribute('data-side-question-id')).toBe('a');
  expect(row.getAttribute('aria-label')).toBe('Expand side question: what is X?');
  expect(row.querySelector('.side-question-dismissed-text')!.textContent).toBe('what is X?');
  // Down on the folded row, up on the open card: the chevron points the way it moves.
  expect(row.querySelector('.side-question-dismissed-chevron polyline')!.getAttribute('points')).toBe('6 9 12 15 18 9');
  act(() => { row.click(); });
  expect(host.querySelector('.side-question-dismiss polyline')!.getAttribute('points')).toBe('6 15 12 9 18 15');
  expect(host.querySelector('[data-role="side-question-card"] .markdown-content')!.textContent!.trim()).toBe('X');
});

describe('placeSideQuestions', () => {
  const exchange = (id: string, userSeq: number): Exchange =>
    ({ userEvent: { _eventId: id } as StoredEvent, userSeq, steps: [] });
  const turn = (id: string): VNode => <div key={`id:${id}`} />;
  const card = (id: string, afterSeq: number | null): SideQuestion =>
    ({ id, threadId: 't1', question: id, afterSeq, dismissed: false, status: 'pending' });
  const order = (nodes: VNode[]) => nodes.map((n) => String(n.key));
  const group = (above: string) => `side-questions:after:${above}`;

  const exchanges = [exchange('e1', 3), exchange('e2', 8), exchange('e3', 12)];
  const turns = [turn('e1'), turn('e2'), turn('e3')];

  it('stays after the turn it was asked under, with later turns below it', () => {
    expect(order(placeSideQuestions(turns, exchanges, [card('q', 9)], false)))
      .toEqual(['id:e1', 'id:e2', group('id:e2'), 'id:e3']);
  });

  it('closes the feed when nothing was said after it', () => {
    expect(order(placeSideQuestions(turns, exchanges, [card('q', 12)], false)))
      .toEqual(['id:e1', 'id:e2', 'id:e3', group('id:e3')]);
  });

  it('closes the feed when asked before any event had loaded', () => {
    expect(order(placeSideQuestions(turns.slice(1), exchanges, [card('q', null)], true)))
      .toEqual(['id:e2', 'id:e3', group('id:e3')]);
  });

  it('draws a message still on its way below the card', () => {
    const pending = exchange('p', Number.MAX_SAFE_INTEGER);
    expect(order(placeSideQuestions([...turns, turn('p')], [...exchanges, pending], [card('q', 12)], false)))
      .toEqual(['id:e1', 'id:e2', 'id:e3', group('id:e3'), 'id:p']);
  });

  it('counts a node that is not one exchange, the queued group, as later', () => {
    const queued = <div key="queued-group" />;
    expect(order(placeSideQuestions([...turns, queued], exchanges, [card('q', 12)], false)))
      .toEqual(['id:e1', 'id:e2', 'id:e3', group('id:e3'), 'queued-group']);
  });

  it('keeps later turns below it past a card pinned out of order', () => {
    const pinned = exchange('e0', 5);
    expect(order(placeSideQuestions([...turns, turn('e0')], [...exchanges, pinned], [card('q', 9)], false)))
      .toEqual(['id:e1', 'id:e2', group('id:e2'), 'id:e3', 'id:e0']);
  });

  it('stacks cards asked at one point into one box, in asking order', () => {
    const placed = placeSideQuestions(turns, exchanges, [card('a', 9), card('b', 10)], false);
    expect(order(placed)).toEqual(['id:e1', 'id:e2', group('id:e2'), 'id:e3']);
    expect((placed[2] as VNode<{ items: SideQuestion[] }>).props.items.map((i) => i.id)).toEqual(['a', 'b']);
  });

  it('keeps the box key when a card is dismissed or a later turn lands', () => {
    const before = order(placeSideQuestions(turns.slice(0, 2), exchanges, [card('a', 9), card('b', 9)], false));
    const after = order(placeSideQuestions(turns, exchanges, [card('b', 9)], false));
    expect(after[2]).toBe(before[2]);
  });

  it('holds back a card asked above the render window', () => {
    expect(order(placeSideQuestions(turns.slice(1), exchanges, [card('q', 4)], true)))
      .toEqual(['id:e2', 'id:e3']);
  });
});
