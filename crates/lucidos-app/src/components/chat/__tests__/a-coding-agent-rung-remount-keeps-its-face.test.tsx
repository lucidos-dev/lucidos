// @vitest-environment jsdom
/**
 * A coding-agent rung control that remounts for a request key it has read
 * starts from that read and refreshes it in place. Otherwise every thread
 * switch and every row unfold would draw the pill again from nothing.
 */
import { beforeAll, describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import type { CodingAgentCommandsResponse } from '../../../api/client';

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

const fetchCodingAgentCommands = vi.fn(() => Promise.resolve(response));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchCodingAgentCommands,
}));

const { RungControl } = await import('../RungControl');
const { controlMenuParts } = await import('../controlMenuParts');
const { threadMap } = await import('../../../store/store');
const { makeThreadState } = await import('../../../store/actions/threads-test-helpers');

beforeAll(() => controlMenuParts.preload());

let host: HTMLDivElement;

async function settle(): Promise<void> {
  for (let i = 0; i < 40; i++) await new Promise(r => setTimeout(r, 0));
}

const mountControl = () => render(<RungControl threadId="t1" codingAgent="claude-code" />, host);
const pillName = () => host.querySelector('.rung-pill .rung-name')?.textContent;

beforeEach(() => {
  vi.clearAllMocks();
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { channel: 'claude_code', status: 'idle' } })]]);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  threadMap.value = new Map();
});

describe('a coding-agent rung control that remounts', () => {
  it('draws its face on the first render and refreshes the read once', async () => {
    mountControl();
    await settle();
    expect(pillName()).toContain('Opus 5.6');

    render(null, host);
    fetchCodingAgentCommands.mockClear();
    mountControl();
    expect(pillName(), 'the first render of the remount').toContain('Opus 5.6');

    await settle();
    expect(fetchCodingAgentCommands).toHaveBeenCalledTimes(1);
    expect(pillName()).toContain('Opus 5.6');
  });
});
