// @vitest-environment jsdom
/**
 * A coding-agent thread can record a model with no effort, because it began
 * before the backend shipped a default effort. When the model offers tiers,
 * the pill still names an effort and never shows the model alone.
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
      { value: 'gpt-6.1-sol', label: 'GPT-6.1 Sol', description: '', reasoning_efforts: ['low', 'medium', 'high'] },
      { value: 'plain-model', label: 'Plain', description: '' },
    ] }],
  }],
  builtin_commands: [],
  skill_commands: [],
  current_model: 'gpt-6.1-sol',
  current_reasoning_effort: null,
  has_active_session: true,
  default_implementer: { model: 'gpt-6.1-sol', effort: 'medium' },
  ladder_rungs: [{ model: 'gpt-6.1-sol', effort: 'medium', label: 'GPT-6.1 Sol', estimated: true }],
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

async function renderLabel(): Promise<string> {
  render(<RungControl threadId="t1" codingAgent="codex" />, host);
  for (let i = 0; i < 40; i++) await new Promise(r => setTimeout(r, 0));
  return document.querySelector('.rung-trigger')!.getAttribute('aria-label')!;
}

beforeEach(() => {
  response = base;
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { channel: 'claude_code', status: 'idle' } })]]);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  threadMap.value = new Map();
});

describe('a recorded model with no effort', () => {
  it('names the default effort when the model offers tiers', async () => {
    expect(await renderLabel()).toBe('Model and effort: GPT-6.1 Sol, default. Choose another');
  });

  it('names the model alone when it offers no tiers', async () => {
    response = { ...base, current_model: 'plain-model' };
    expect(await renderLabel()).toBe('Model: plain-model. Choose another');
  });
});
