// @vitest-environment jsdom
/** A typed answer the engine has not confirmed shows on the card it answers.
 *  While it sends, the header reads "Sending" and, past the delay gate, the
 *  "Your answer" label spins. When its POST got no answer, the card goes live
 *  again and the answer shows "Not sent" with a retry icon. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, type ComponentChildren } from 'preact';
import { act } from 'preact/test-utils';

const { retryUnsentMessage, discardUnsentAnswers } = vi.hoisted(() => ({
  retryUnsentMessage: vi.fn(),
  discardUnsentAnswers: vi.fn(),
}));
vi.mock('../../../store/actions/chat', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/chat')>()),
  retryUnsentMessage,
  discardUnsentAnswers,
}));
vi.mock('../../../store/actions/chat-claude-code', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/chat-claude-code')>()),
  answerThreadQuestion: vi.fn().mockResolvedValue('sent'),
}));

// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, join } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';
import { QuestionBody } from '../QuestionCard';
import { pendingAnswers } from '../../../store/pendingDecisions';
import { unsentMessages, type UnsentMessage } from '../../../store/unsentMessages';
import { describeInitiator } from '../ChatExchange';
import type { Exchange, TypedAnswer } from '../../../store/thread-events';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
  retryUnsentMessage.mockReset();
  discardUnsentAnswers.mockReset();
});

afterEach(() => {
  render(null, host);
  host.remove();
  pendingAnswers.clear('tu-1');
  vi.useRealTimers();
});

function mount(node: ComponentChildren) {
  act(() => { render(node, host); });
}

const options = [{ id: 'a', label: 'Skip it' }, { id: 'b', label: 'Wait' }];
const sending: TypedAnswer = { state: 'sending', text: 'neither, do C', image_hashes: [] };
const unsent: TypedAnswer = { state: 'unsent', text: 'neither, do C', image_hashes: [], unsentEventId: 'e-1' };

const card = (typedAnswer?: TypedAnswer, terminated = false) => (
  <QuestionBody threadId="t" toolUseId="tu-1" question="Wait for the lock?" options={options} typedAnswer={typedAnswer} terminated={terminated} />
);

const header = (typedAnswer: TypedAnswer) => {
  const exchange: Exchange = {
    userEvent: { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: 's', question: 'q', options } as Exchange['userEvent'],
    userSeq: 0,
    steps: [],
    typedAnswer,
  };
  return describeInitiator(exchange, '', [], 't', false, true).status;
};

describe('a typed answer still sending', () => {
  it('shows as the card\'s answer and spins only past the delay gate', () => {
    mount(card(sending));
    expect(host.querySelector('.question-body-answered')).not.toBeNull();
    expect(host.querySelector('.question-freetext-text')!.textContent!.trim()).toBe('neither, do C');
    expect(host.querySelector('.question-freetext-sending')).toBeNull();
    act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
    expect(host.querySelector('.question-freetext-label .mini-spinner.question-freetext-sending')).not.toBeNull();
  });

  it('stops spinning once the answer lands', () => {
    mount(card(sending));
    act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
    mount(
      <QuestionBody threadId="t" toolUseId="tu-1" question="Wait for the lock?" options={options}
        resolved={{ kind: 'FreeText', text: 'neither, do C', image_hashes: [] }} />,
    );
    expect(host.querySelector('.question-freetext-text')!.textContent!.trim()).toBe('neither, do C');
    expect(host.querySelector('.question-freetext-sending')).toBeNull();
  });

  it('goes live again when the answer leaves without landing', () => {
    mount(card(sending));
    mount(card());
    expect(host.querySelector('.question-body-answered')).toBeNull();
    expect(host.querySelector('button.question-option')).not.toBeNull();
  });

  it('makes the divider header read "Sending"', () => {
    mount(header(sending));
    expect(host.textContent).toBe('Sending');
  });
});

describe('a typed answer that was not sent', () => {
  it('opens what its attempts ended on when Not sent is tapped', () => {
    unsentMessages.value = new Map([['e-1', { failure: { attempts: 4, reason: 'request timed out' } } as UnsentMessage]]);
    mount(card(unsent));
    act(() => { host.querySelector<HTMLButtonElement>('.question-unsent-toggle')!.click(); });
    expect(host.querySelector('.question-unsent-detail')!.textContent)
      .toBe('Lucidos did not answer after 4 tries. Last error: request timed out.');
    unsentMessages.value = new Map();
  });

  it('keeps the options live and shows "Not sent" with a retry icon under the answer', () => {
    mount(card(unsent));
    expect(host.querySelectorAll('button.question-option:not(:disabled)')).toHaveLength(2);
    expect(host.querySelector('.question-freetext-text')!.textContent!.trim()).toBe('neither, do C');
    expect(host.querySelector('.question-unsent-row')!.textContent).toContain('Not sent');
    const retry = host.querySelector<HTMLButtonElement>('button.question-unsent-retry')!;
    expect(retry.getAttribute('aria-label')).toBe('Retry sending your answer');
    expect(retry.querySelector('svg')).not.toBeNull();
    act(() => { retry.click(); });
    expect(retryUnsentMessage).toHaveBeenCalledWith('e-1');
  });

  it('is replaced when the user taps an option instead', async () => {
    mount(card(unsent));
    await act(async () => { host.querySelector<HTMLElement>('button.question-option .question-option-label')!.click(); });
    expect(discardUnsentAnswers).toHaveBeenCalledWith('t', 'tu-1');
  });

  it('is replaced on every way this device answers a card', () => {
    // A multi-select submit lives in PromptInput, outside this component.
    const src = (name: string): string => readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', name), 'utf8');
    expect(src('QuestionCard.tsx')).toContain('discardUnsentAnswers(threadId, toolUseId)');
    expect(src('PromptInput.tsx')).toContain('discardUnsentAnswers(focused, pendingMultiQ.toolUseId)');
  });

  it('stays visible on a card whose turn ended', () => {
    mount(card(unsent, true));
    expect(host.querySelector('.question-body-terminated')).not.toBeNull();
    expect(host.querySelector('.question-unsent-row')).not.toBeNull();
  });

  it('makes the divider header read "Not sent"', () => {
    mount(header(unsent));
    expect(host.textContent).toBe('Not sent');
    expect(host.querySelector('.exchange-status-not-sent')).not.toBeNull();
  });
});
