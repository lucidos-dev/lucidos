// @vitest-environment jsdom
/**
 * A rung pick shuts the list at once, before its control request answers. An
 * open list keeps the UI behind inert, so a Send tapped during that round trip
 * only closed the list and the message never went.
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
      { value: 'claude-opus-5-5', label: 'Opus 5.5', description: '' },
    ] }],
  }],
  builtin_commands: [],
  skill_commands: [],
  current_model: 'default',
  current_reasoning_effort: null,
  has_active_session: true,
  ladder_rungs: [{ model: 'claude-opus-5-5', effort: 'medium', label: 'Opus 5.5', estimated: true }],
} as unknown as CodingAgentCommandsResponse;

let answerControl: ((recorded: boolean) => void) | null = null;
const sendCodingAgentControl = vi.fn(() => new Promise<boolean>((resolve) => { answerControl = resolve; }));
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

// The panel parts are code-split. Warm them, so a press opens at once.
beforeAll(() => controlMenuParts.preload());

let host: HTMLDivElement;

async function settle(): Promise<void> {
  for (let i = 0; i < 40; i++) await new Promise(r => setTimeout(r, 0));
}

function click(selector: string, text?: string): void {
  const el = Array.from(document.querySelectorAll<HTMLElement>(selector))
    .find(e => text === undefined || e.textContent?.includes(text));
  if (!el) throw new Error(`no ${selector}${text ? ` with "${text}"` : ''}`);
  el.click();
}

beforeEach(() => {
  answerControl = null;
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('a rung pick on a started thread', () => {
  it('frees the UI behind before its control request answers', async () => {
    render(<RungControl threadId="t1" codingAgent="claude-code" />, host);
    await settle();
    click('.rung-trigger');
    await settle();
    click('.control-option', 'Opus 5.5 · medium');
    await settle();

    expect(sendCodingAgentControl).toHaveBeenCalledTimes(1);
    expect(answerControl, 'the control request already answered').not.toBeNull();
    expect(document.documentElement.hasAttribute('data-overlay-open'),
      'the list still holds the UI behind inert').toBe(false);

    answerControl!(true);
    await settle();
  });
});
