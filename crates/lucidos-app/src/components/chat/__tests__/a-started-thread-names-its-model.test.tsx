// @vitest-environment jsdom
/**
 * A started coding-agent thread with no model of its own still names one.
 * Right after a send, before the session publishes its pair, the pill names
 * the default implementer, never "Default". With no default set, it names the
 * backend, whose own config picks.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import type { CodingAgentCommandsResponse } from '../../../api/client';

const base = {
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
  current_model: null,
  current_reasoning_effort: null,
  has_active_session: true,
  default_implementer: { model: 'claude-opus-5-6', effort: 'medium' },
  ladder_rungs: [
    { model: 'claude-opus-5-6', effort: 'medium', label: 'Opus 5.6', estimated: true },
    { model: 'claude-opus-5-6', effort: 'high', label: 'Opus 5.6', estimated: true },
  ],
};

let response: Record<string, unknown> = base;

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchCodingAgentCommands: vi.fn(() => Promise.resolve(response as unknown as CodingAgentCommandsResponse)),
}));

const { RungControl } = await import('../RungControl');
const { threadMap } = await import('../../../store/store');
const { makeThreadState } = await import('../../../store/actions/threads-test-helpers');

let host: HTMLDivElement;

async function renderStarted(codingAgent: 'claude-code' | 'codex'): Promise<string> {
  render(<RungControl threadId="t1" codingAgent={codingAgent} />, host);
  for (let i = 0; i < 40; i++) await new Promise(r => setTimeout(r, 0));
  return document.querySelector('.rung-trigger')!.getAttribute('aria-label')!;
}

beforeEach(() => {
  response = base;
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { channel: 'claude_code', status: 'running' } })]]);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  threadMap.value = new Map();
});

describe('a started thread with no model of its own', () => {
  it('names the default implementer', async () => {
    expect(await renderStarted('claude-code')).toBe('Model and effort: Opus 5.6, medium, rung 1 of 2. Choose another');
  });

  it("keeps the thread's own effort beside the default model", async () => {
    response = { ...base, current_reasoning_effort: 'high' };
    expect(await renderStarted('claude-code')).toBe('Model and effort: Opus 5.6, high, rung 2 of 2. Choose another');
  });

  it('names the backend when no default implementer is set', async () => {
    response = { ...base, default_implementer: null };
    expect(await renderStarted('codex')).toBe('Model: Codex. Choose another');
  });
});
