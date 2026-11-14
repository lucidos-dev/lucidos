// @vitest-environment jsdom
/** Settings › Intelligence: a ladder that does not climb offers Sort by
 *  intelligence, and a customised one wears a mark. Each shows only when it
 *  holds, and the button asks the engine to sort. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { CodingAgent, IntelligenceLadder, LadderSurface } from '../../../api/types';

const ladder = (surface: LadderSurface, flags: { customised: boolean; climbs: boolean }): IntelligenceLadder => ({
  surface,
  source: 'test',
  estimated: '2026-10-16',
  ...flags,
  shipped_rungs: [],
  default_rung: null,
  reference: null,
  long_prompt_share: null,
  rungs: [],
});

const { getCodingAgentLadder, sortLadder } = vi.hoisted(() => ({
  getCodingAgentLadder: vi.fn(),
  sortLadder: vi.fn(async (_surface: LadderSurface) => ({ rungs: [] })),
}));
getCodingAgentLadder.mockImplementation(async (agent: CodingAgent) =>
  ladder(agent, agent === 'claude-code' ? { customised: true, climbs: false } : { customised: false, climbs: true }));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  getChatLadder: async () => ladder('chat', { customised: false, climbs: true }),
  getCodingAgentLadder,
  getModelPrices: async () => ({ checked: '2026-10-16' }),
  sortLadder,
}));

import { IntelligencePage } from '../IntelligencePage';

let host: HTMLDivElement;

async function mount() {
  host = document.createElement('div');
  document.body.appendChild(host);
  await act(async () => { render(<IntelligencePage />, host); });
  await act(async () => { await Promise.resolve(); });
}

beforeEach(() => {
  sortLadder.mockClear();
});

afterEach(() => {
  render(null, host);
  host.remove();
});

const section = (name: string) =>
  [...host.querySelectorAll('.ladder-section')].find((s) => s.querySelector('.ladder-title')?.textContent?.includes(name))!;

it('shows the notice and the mark only on the ladder that earns each', async () => {
  await mount();
  const claude = section('Claude Code');
  expect(claude.querySelector('.ladder-notice')?.textContent).toContain('a more intelligent rung sits above a less intelligent one');
  expect(claude.querySelector('.ladder-customised')?.textContent).toBe('Customised');
  for (const other of ['Lucidos Agent', 'Codex']) {
    expect(section(other).querySelector('.ladder-notice')).toBeNull();
    expect(section(other).querySelector('.ladder-customised')).toBeNull();
  }
});

it('asks the engine to sort that ladder when Sort by intelligence is tapped', async () => {
  await mount();
  const button = section('Claude Code').querySelector('.ladder-notice button') as HTMLButtonElement;
  expect(button.textContent).toBe('Sort by intelligence');
  await act(async () => { button.click(); });
  expect(sortLadder).toHaveBeenCalledExactlyOnceWith('claude-code');
});
