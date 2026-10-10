// @vitest-environment jsdom
/** A tapped answer whose quiet retries all went unanswered stays on its card:
 *  the picked option reads as picked, and "Not sent" with a retry icon sits on
 *  it. Tapping "Not sent" opens what the attempts ended on. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, type ComponentChildren } from 'preact';
import { act } from 'preact/test-utils';

const { retryUnsentPick, discardUnsentAnswers } = vi.hoisted(() => ({
  retryUnsentPick: vi.fn(),
  discardUnsentAnswers: vi.fn(),
}));
vi.mock('../../../store/actions/chat-claude-code', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/chat-claude-code')>()),
  retryUnsentPick,
}));
vi.mock('../../../store/actions/chat', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/chat')>()),
  discardUnsentAnswers,
}));

import { QuestionBody } from '../QuestionCard';
import { AwaitingStatus } from '../ChatExchange';
import { pendingAnswers, unsentPicks } from '../../../store/pendingDecisions';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  retryUnsentPick.mockReset();
  discardUnsentAnswers.mockReset();
});

afterEach(() => {
  render(null, host);
  host.remove();
  unsentPicks.clear('tu-1');
});

function mount(node: ComponentChildren) {
  act(() => { render(node, host); });
}

const options = [{ id: 'a', label: 'Skip it' }, { id: 'b', label: 'Wait' }, { id: 'c', label: 'Ask later' }];
const failure = { attempts: 4, reason: 'Load failed' };

const card = (multiSelect = false) => (
  <QuestionBody threadId="t" toolUseId="tu-1" question="Wait for the lock?" options={options} multiSelect={multiSelect} />
);

function optionWithNotice(): HTMLElement | null {
  return host.querySelector<HTMLElement>('.question-option-unsent');
}

describe('a tapped answer that was not sent', () => {
  it('marks the picked option and puts Not sent with a retry icon on it', () => {
    unsentPicks.set('tu-1', { threadId: 't', answer: { kind: 'Selected', option_id: 'b' }, failure });
    mount(card());

    const wrapper = optionWithNotice()!;
    expect(wrapper.querySelector('.question-option-label')!.textContent).toBe('Wait');
    expect(wrapper.querySelector('.question-option')!.classList.contains('question-option-selected')).toBe(true);
    expect(wrapper.querySelector('.question-unsent-toggle')!.textContent).toBe('Not sent');
    expect(wrapper.querySelector('.question-unsent-retry')).not.toBeNull();
    // The notice is beside the option button, never inside it.
    expect(wrapper.querySelector('.question-option .question-unsent')).toBeNull();
    // The other options stay live.
    const live = [...host.querySelectorAll<HTMLButtonElement>('button.question-option')].filter(b => !b.disabled);
    expect(live).toHaveLength(3);
  });

  it('opens the error details when Not sent is tapped', () => {
    unsentPicks.set('tu-1', { threadId: 't', answer: { kind: 'Selected', option_id: 'b' }, failure });
    mount(card());

    const toggle = host.querySelector<HTMLButtonElement>('.question-unsent-toggle')!;
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    act(() => { toggle.click(); });
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(host.querySelector('.question-unsent-detail')!.textContent)
      .toBe('Lucidos did not answer after 4 tries. Last error: Load failed.');
  });

  it('sends the same answer again from the retry icon', () => {
    unsentPicks.set('tu-1', { threadId: 't', answer: { kind: 'Selected', option_id: 'b' }, failure });
    mount(card());

    act(() => { host.querySelector<HTMLButtonElement>('.question-unsent-retry')!.click(); });
    expect(retryUnsentPick).toHaveBeenCalledWith('tu-1');
  });

  it('a multi-select answer marks every picked option, with the notice on the first', () => {
    unsentPicks.set('tu-1', { threadId: 't', answer: { kind: 'MultiSelected', option_ids: ['c', 'a'] }, failure });
    mount(card(true));

    expect(host.querySelectorAll('.question-option-unsent')).toHaveLength(1);
    expect(optionWithNotice()!.querySelector('.question-option-label')!.textContent).toBe('Skip it');
    const marked = [...host.querySelectorAll('.question-option-selected .question-option-label')].map(l => l.textContent);
    expect(marked).toEqual(['Skip it', 'Ask later']);
  });

  it('a multi-select answer shows the text it typed, so its Retry sends nothing unseen', () => {
    unsentPicks.set('tu-1', { threadId: 't', answer: { kind: 'MultiSelected', option_ids: ['a'], text: 'and hurry' }, failure });
    mount(card(true));

    expect(host.querySelector('.question-freetext-text')!.textContent!.trim()).toBe('and hurry');
    // The notice stays on the ticked option.
    expect(host.querySelectorAll('.question-unsent')).toHaveLength(1);
    expect(optionWithNotice()!.querySelector('.question-unsent')).not.toBeNull();
  });

  it('a multi-select answer with only text puts Not sent on its text', () => {
    unsentPicks.set('tu-1', { threadId: 't', answer: { kind: 'MultiSelected', option_ids: [], text: 'none of these' }, failure });
    mount(card(true));

    expect(optionWithNotice()).toBeNull();
    const block = host.querySelector('.question-freetext')!;
    expect(block.querySelector('.question-freetext-text')!.textContent!.trim()).toBe('none of these');
    act(() => { block.querySelector<HTMLButtonElement>('.question-unsent-retry')!.click(); });
    expect(retryUnsentPick).toHaveBeenCalledWith('tu-1');
  });

  it('a retry that lands replaces a typed answer to the card that was not sent', async () => {
    unsentPicks.set('tu-1', { threadId: 't', answer: { kind: 'Selected', option_id: 'b' }, failure });
    retryUnsentPick.mockResolvedValue('sent');
    mount(card());

    await act(async () => { host.querySelector<HTMLButtonElement>('.question-unsent-retry')!.click(); });
    expect(discardUnsentAnswers).toHaveBeenCalledWith('t', 'tu-1');
  });

  it('is cleared once the card\'s answer is recorded', () => {
    unsentPicks.set('tu-1', { threadId: 't', answer: { kind: 'Selected', option_id: 'b' }, failure });
    mount(<QuestionBody threadId="t" toolUseId="tu-1" question="Wait for the lock?" options={options} resolved={{ kind: 'Selected', option_id: 'a' }} />);
    expect(unsentPicks.map.value.has('tu-1')).toBe(false);
  });

  it('the card header reads Not sent', () => {
    unsentPicks.set('tu-1', { threadId: 't', answer: { kind: 'Selected', option_id: 'b' }, failure });
    mount(<AwaitingStatus picks={pendingAnswers.map} id="tu-1" unsent={unsentPicks.map} />);
    expect(host.textContent).toBe('Not sent');
  });
});
