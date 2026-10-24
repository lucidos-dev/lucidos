// @vitest-environment jsdom
/**
 * A started coding-agent thread keeps its rung while a turn runs (ADR 0542).
 * The pill dims and says why, a press opens no list and records nothing, and
 * the pill frees again once the thread is idle.
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
const fetchSwitchEstimate = vi.fn(() => Promise.resolve({ context_tokens: 1000, rebuilds_cache: false, extra_cost_usd: null }));

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

const pill = () => document.querySelector<HTMLElement>('.rung-pill')!;
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

describe('the rung pill on a started coding-agent thread', () => {
  it.each(LOCKED)('is locked while the thread is %s', async (_state, seed) => {
    seed();
    render(<RungControl threadId="t1" codingAgent="claude-code" />, host);
    await settle();

    expect(pill().hasAttribute('data-locked')).toBe(true);
    expect(trigger().getAttribute('aria-disabled')).toBe('true');
    expect(trigger().getAttribute('data-tooltip')).toMatch(/while a turn runs/);

    trigger().click();
    document.querySelector<HTMLElement>('.rung-step:last-child')!.click();
    await settle();
    expect(document.querySelector('.control-option'), 'the list opened').toBeNull();
    expect(sendCodingAgentControl).not.toHaveBeenCalled();
    expect(fetchSwitchEstimate).not.toHaveBeenCalled();
    expect(toasts.value.map(t => t.message).join(' ')).toMatch(/while a turn runs/);
  });

  it('frees once the thread is idle, and a pick records on the engine', async () => {
    seedThread('running');
    render(<RungControl threadId="t1" codingAgent="claude-code" />, host);
    await settle();
    expect(pill().hasAttribute('data-locked')).toBe(true);

    seedThread('idle');
    await settle();
    expect(pill().hasAttribute('data-locked')).toBe(false);
    expect(trigger().hasAttribute('aria-disabled')).toBe(false);

    trigger().click();
    await settle();
    Array.from(document.querySelectorAll<HTMLElement>('.control-option'))
      .find(e => e.textContent?.includes('Opus 5.6 · high'))!.click();
    await settle();
    expect(sendCodingAgentControl).toHaveBeenCalledTimes(1);
    expect(sendCodingAgentControl).toHaveBeenCalledWith('t1', { subtype: 'set_model', model: 'claude-opus-5-6', effort: 'high' });
  });
});
