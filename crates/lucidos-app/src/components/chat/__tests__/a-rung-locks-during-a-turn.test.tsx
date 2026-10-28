// @vitest-environment jsdom
/**
 * A started coding-agent thread keeps its rung while a turn runs (ADR 0542).
 * The pill still opens its list, explains the lock and records nothing.
 * Once the thread is idle a pick records again.
 */
import { beforeAll, describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import type { CodingAgentCommandsResponse } from '../../../api/client';
import type { PendingUserMessage, ThreadStatus } from '../../../store/thread-events';

const response: CodingAgentCommandsResponse = {
  control_commands: [{
    subtype: 'set_model',
    label: 'Model',
    params: [{ key: 'model', options: [
      { value: 'default', label: 'Default', description: '' },
      { value: 'claude-opus-5-6', label: 'Opus 5.6', description: '', reasoning_efforts: ['medium', 'high'] },
    ] }],
  }, {
    subtype: 'set_reasoning_effort',
    label: 'Reasoning effort',
    params: [{ key: 'effort', options: [
      { value: 'medium', label: 'Medium', description: 'Balanced' },
      { value: 'high', label: 'High', description: 'Deeper' },
    ] }],
  }],
  builtin_commands: [],
  skill_commands: [],
  current_model: 'claude-opus-5-6',
  current_reasoning_effort: 'medium',
  has_active_session: true,
  default_implementer: { model: 'claude-opus-5-6', effort: 'medium' },
  ladder_rungs: [
    { model: 'claude-opus-5-6', effort: 'medium', label: 'Opus 5.6', estimated: true },
    { model: 'claude-opus-5-6', effort: 'high', label: 'Opus 5.6', estimated: true },
  ],
} as unknown as CodingAgentCommandsResponse;

const sendCodingAgentControl = vi.fn(() => Promise.resolve(true));
const fetchSwitchEstimate = vi.fn(() => Promise.resolve({ context_tokens: 1000, rebuilds_cache: 'no' as const, extra_cost_usd: null }));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchCodingAgentCommands: vi.fn(() => Promise.resolve(response)),
  fetchSwitchEstimate,
}));
vi.mock('../../../store/actions/chat-claude-code', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/chat-claude-code')>()),
  sendCodingAgentControl,
}));

const { RungControl } = await import('../RungControl');
const { controlMenuParts } = await import('../controlMenuParts');
const { threadMap, toasts } = await import('../../../store/store');
const { makeThreadState } = await import('../../../store/actions/threads-test-helpers');

beforeAll(() => controlMenuParts.preload());

let host: HTMLDivElement;

async function settle(): Promise<void> {
  for (let i = 0; i < 40; i++) await new Promise(r => setTimeout(r, 0));
}

function seedThread(status: ThreadStatus, pendingUserMessages: PendingUserMessage[] = []) {
  threadMap.value = new Map([['t1', makeThreadState('t1', {
    meta: { channel: 'claude_code', status },
    pendingUserMessages,
  })]]);
}

const trigger = () => document.querySelector<HTMLElement>('.rung-trigger')!;

beforeEach(() => {
  vi.clearAllMocks();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  threadMap.value = new Map();
  toasts.value = [];
});

const LOCKED: [string, () => void][] = [
  ['requesting', () => seedThread('idle', [{ text: 'Fix it', eventId: 'e1', created: '2026-01-01T00:00:00Z' }])],
  ['working', () => seedThread('running')],
  ['awaiting an answer', () => seedThread('waiting_for_user_answer')],
  ['paused', () => seedThread('paused')],
];

async function pickHigh(): Promise<void> {
  trigger().click();
  await settle();
  Array.from(document.querySelectorAll<HTMLElement>('.control-option'))
    .find(e => e.textContent?.includes('Opus 5.6 · high'))!.click();
  await settle();
}

const lockToasts = () => toasts.value.filter(t => t.type === 'info' && /while a turn runs/.test(t.message));

describe('the rung pill on a started coding-agent thread', () => {
  it.each(LOCKED)('opens but refuses a pick while the thread is %s', async (_state, seed) => {
    seed();
    render(<RungControl threadId="t1" codingAgent="claude-code" />, host);
    await settle();
    expect(trigger().getAttribute('aria-disabled')).toBe('false');
    expect(trigger().getAttribute('data-tooltip')).toMatch(/while a turn runs/);
    expect(host.querySelector('.rung-name')!.textContent).toBe('Opus 5.6');
    await pickHigh();
    expect(lockToasts()).toHaveLength(1);
    toasts.value = [];
    document.querySelector<HTMLElement>('.rung-step:last-child')!.click();
    await settle();
    expect(lockToasts()).toHaveLength(1);
    expect(sendCodingAgentControl).not.toHaveBeenCalled();
    expect(fetchSwitchEstimate).not.toHaveBeenCalled();
  });

  it('keeps an open list open when a turn starts, and refuses its pick', async () => {
    seedThread('idle');
    render(<RungControl threadId="t1" codingAgent="claude-code" />, host);
    await settle();
    trigger().click();
    await settle();
    seedThread('running');
    await settle();
    expect(trigger().getAttribute('aria-expanded')).toBe('true');
    Array.from(document.querySelectorAll<HTMLElement>('.control-option'))
      .find(e => e.textContent?.includes('Opus 5.6 · high'))!.click();
    await settle();
    expect(lockToasts()).toHaveLength(1);
    expect(sendCodingAgentControl).not.toHaveBeenCalled();
  });

  it('keeps keyboard focus through a model change', async () => {
    seedThread('idle');
    render(<RungControl threadId="t1" codingAgent="claude-code" />, host);
    await settle();
    let complete!: (ok: boolean) => void;
    sendCodingAgentControl.mockImplementationOnce(() => new Promise<boolean>(resolve => { complete = resolve; }));
    const button = trigger();
    button.focus();
    button.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    await settle();
    expect(button.getAttribute('aria-disabled')).toBe('true');
    expect(document.activeElement).toBe(button);
    complete(true);
    await settle();
    expect(button.getAttribute('aria-disabled')).toBe('false');
    expect(document.activeElement).toBe(button);
  });

  it('records a pick on the engine once the thread is idle', async () => {
    seedThread('running');
    render(<RungControl threadId="t1" codingAgent="claude-code" />, host);
    await settle();

    seedThread('idle');
    await settle();
    await pickHigh();
    expect(sendCodingAgentControl).toHaveBeenCalledTimes(1);
    expect(sendCodingAgentControl).toHaveBeenCalledWith('t1', { subtype: 'set_model', model: 'claude-opus-5-6', effort: 'high' });
  });
});
