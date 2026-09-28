// @vitest-environment jsdom
/** The `/btw` side-question card (ADR 0318): the question, then a delayed
 *  "Thinking", the answer as markdown, or the error. It dismisses, and it
 *  always says the answer is not part of the conversation. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { SIDE_QUESTION_NOTE, SIDE_QUESTION_THINKING_DELAY_MS, SideQuestionCards } from '../SideQuestionCard';
import { sideQuestions, type SideQuestion } from '../../../store/sideQuestions';

let host: HTMLDivElement;

function show(items: SideQuestion[]) {
  act(() => {
    sideQuestions.value = items;
    render(<SideQuestionCards threadId="t1" />, host);
  });
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  sideQuestions.value = [];
  vi.useRealTimers();
});

it('renders nothing for a thread without side questions', () => {
  show([{ id: 'a', threadId: 'other', question: 'q', status: 'pending' }]);
  expect(host.innerHTML).toBe('');
});

it('renders the answer as markdown, with the not-recorded note', () => {
  show([{ id: 'a', threadId: 't1', question: 'what is X?', status: 'answered', answer: 'X is **bold**.' }]);
  const card = host.querySelector('[data-role="side-question-card"]')!;
  expect(card.querySelector('.side-question-question')!.textContent).toBe('what is X?');
  expect(card.querySelector('.markdown-content strong')!.textContent).toBe('bold');
  expect(card.textContent).toContain(SIDE_QUESTION_NOTE);
});

it('says Thinking only after a delay while pending', () => {
  vi.useFakeTimers();
  show([{ id: 'a', threadId: 't1', question: 'q', status: 'pending' }]);
  const thinking = host.querySelector('.side-question-thinking')!;
  expect(thinking.textContent).toBe('');
  act(() => { vi.advanceTimersByTime(SIDE_QUESTION_THINKING_DELAY_MS); });
  expect(host.querySelector('.side-question-thinking')!.textContent).toBe('Thinking…');
});

it('shows a failure as an alert', () => {
  show([{ id: 'a', threadId: 't1', question: 'q', status: 'failed', error: 'Side questions are not available in Codex threads.' }]);
  const alert = host.querySelector('[role="alert"]')!;
  expect(alert.textContent).toBe('Side questions are not available in Codex threads.');
});

it('dismisses one card from a stack with a labelled button', () => {
  show([
    { id: 'a', threadId: 't1', question: 'first', status: 'answered', answer: '1' },
    { id: 'b', threadId: 't1', question: 'second', status: 'answered', answer: '2' },
  ]);
  const buttons = host.querySelectorAll<HTMLButtonElement>('button[aria-label="Dismiss side question"]');
  expect(buttons).toHaveLength(2);
  expect(buttons[0].getAttribute('data-tooltip')).toBe('Dismiss side question');
  act(() => { buttons[0].click(); });
  const questions = [...host.querySelectorAll('.side-question-question')].map((n) => n.textContent);
  expect(questions).toEqual(['second']);
});
