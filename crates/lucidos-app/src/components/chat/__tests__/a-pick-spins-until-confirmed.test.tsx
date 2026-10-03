// @vitest-environment jsdom
/** A card the user has answered says so before the engine confirms it. On a
 *  slow link or a busy host that wait can last seconds. The header reads
 *  "Sending" for the whole wait. Past the delay gate, the picked option or
 *  button also spins in place of its mark. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, type ComponentChildren } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/chat-claude-code', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/chat-claude-code')>()),
  answerThreadQuestion: () => new Promise(() => {}),
}));
vi.mock('../../../store/actions/permissions', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/permissions')>()),
  resolveCodingAgentPermission: () => new Promise(() => {}),
}));

import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';
import { QuestionBody, pendingAnswers } from '../QuestionCard';
import { PermissionBody, pendingVerdicts } from '../PermissionCard';
import { describeInitiator } from '../ChatExchange';
import type { Exchange } from '../../../store/thread-events';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  pendingAnswers.clear('tu-1');
  pendingVerdicts.clear('req-1');
  vi.useRealTimers();
});

function mount(node: ComponentChildren) {
  act(() => { render(node, host); });
}

const options = [{ id: 'a', label: 'Skip it' }, { id: 'b', label: 'Wait' }];

const question = (resolved?: { kind: 'Selected'; option_id: string }, terminated = false) => (
  <QuestionBody
    threadId="t"
    toolUseId="tu-1"
    question="Wait for the lock?"
    options={options}
    resolved={resolved}
    terminated={terminated}
  />
);

describe('a question card', () => {
  it('holds a tapped option in the shared picks, where the header can read it', () => {
    mount(question());
    act(() => { host.querySelector<HTMLElement>('button.question-option .question-option-label')!.click(); });
    expect(pendingAnswers.map.value.get('tu-1')).toEqual({ kind: 'Selected', option_id: 'a' });
  });

  it('spins the picked indicator only past the delay gate', () => {
    pendingAnswers.set('tu-1', { kind: 'Selected', option_id: 'a' });
    mount(question());
    expect(host.querySelector('.question-option-sending')).toBeNull();
    act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
    const picked = host.querySelector('.question-option-selected')!;
    expect(picked.querySelector('.mini-spinner.question-option-sending')).not.toBeNull();
    expect(picked.querySelector('.question-option-indicator')).toBeNull();
    expect(host.querySelector('.question-option-dimmed .question-option-sending')).toBeNull();
  });

  it('settles to the filled dot and drains the pick once the answer lands', () => {
    pendingAnswers.set('tu-1', { kind: 'Selected', option_id: 'a' });
    mount(question());
    act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
    mount(question({ kind: 'Selected', option_id: 'a' }));
    expect(host.querySelector('.question-option-sending')).toBeNull();
    expect(host.querySelector('.question-option-selected .question-option-indicator-selected')).not.toBeNull();
    expect(pendingAnswers.map.value.has('tu-1')).toBe(false);
  });

  it('draws a dead card without the unconfirmed pick, and keeps it for a late answer', () => {
    pendingAnswers.set('tu-1', { kind: 'Selected', option_id: 'a' });
    mount(question(undefined, true));
    act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
    expect(host.querySelector('.question-option-sending')).toBeNull();
    expect(host.querySelector('.question-body-terminated')).not.toBeNull();
    expect(pendingAnswers.map.value.has('tu-1')).toBe(true);

    mount(question());
    expect(host.querySelector('.question-option-selected')).not.toBeNull();
  });
});

const permissionEvent = { request_id: 'req-1', tool_use_id: 'tu-p', tool_name: 'Bash', input: { command: 'ls' }, summary: 'list files' };

describe('a permission card', () => {
  it('spins in place of the check on the picked button, then shows the check on confirm', () => {
    mount(<PermissionBody event={permissionEvent} />);
    act(() => { host.querySelector<HTMLButtonElement>('button[aria-label="Allow this permission request once"]')!.click(); });
    expect(pendingVerdicts.map.value.get('req-1')).toEqual({ allowed: true, persist_scope: undefined });
    act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
    const picked = host.querySelector('.permission-btn-picked')!;
    expect(picked.querySelector('.mini-spinner.permission-btn-sending')).not.toBeNull();
    expect(picked.querySelector('.permission-btn-check')).toBeNull();

    mount(<PermissionBody event={permissionEvent} resolved={{ allowed: true }} />);
    expect(host.querySelector('.permission-btn-sending')).toBeNull();
    expect(host.querySelector('.permission-btn-picked .permission-btn-check')).not.toBeNull();
    expect(pendingVerdicts.map.value.has('req-1')).toBe(false);
  });

  it('draws a dead card without the unconfirmed pick', () => {
    pendingVerdicts.set('req-1', { allowed: true });
    mount(<PermissionBody event={permissionEvent} terminated />);
    expect(host.querySelector('.permission-btn-picked')).toBeNull();
    expect(host.querySelector('.permission-body-terminated')).not.toBeNull();
  });
});

const asked: Exchange = {
  userEvent: {
    type: 'UserQuestionAsked',
    tool_use_id: 'tu-1',
    cc_session_id: 'sess',
    question: 'Wait for the lock?',
    options,
  } as Exchange['userEvent'],
  userSeq: 0,
  steps: [],
};

const permissionAsked: Exchange = {
  userEvent: { type: 'CodingAgentPermissionRequest', ...permissionEvent } as Exchange['userEvent'],
  userSeq: 0,
  steps: [],
};

describe('the divider header', () => {
  it.each([
    ['question', asked, () => pendingAnswers.set('tu-1', { kind: 'Selected', option_id: 'a' })],
    ['permission', permissionAsked, () => pendingVerdicts.set('req-1', { allowed: true })],
  ])('reads "Sending" while a %s pick is in flight', (_kind, exchange, pick) => {
    mount(describeInitiator(exchange, '', [], 't', false, true).status);
    expect(host.textContent).toBe('Needs your answer');
    act(pick);
    expect(host.textContent).toBe('Sending');
    expect(host.querySelector('.exchange-status-sending')).not.toBeNull();
  });
});
