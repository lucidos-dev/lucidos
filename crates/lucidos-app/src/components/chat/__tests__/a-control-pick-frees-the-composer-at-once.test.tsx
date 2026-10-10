// @vitest-environment jsdom
/**
 * A pick in the coding-agent menu shuts the menu at once, before its control
 * request answers. An open menu keeps the UI behind inert, so a Send tapped
 * during that round trip only closed the menu and the message never went.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import type { CodingAgentCommandsResponse } from '../../../api/client';

const response: CodingAgentCommandsResponse = {
  control_commands: [{
    subtype: 'set_permission_mode',
    label: 'Permission mode',
    params: [{ key: 'mode', options: [{ value: 'plan', label: 'Plan' }] }],
  }],
  builtin_commands: [],
  skill_commands: [],
  has_active_session: true,
} as unknown as CodingAgentCommandsResponse;

let answerControl: ((result: 'ok') => void) | null = null;
const sendCodingAgentControl = vi.fn(() => new Promise<'ok'>((resolve) => { answerControl = resolve; }));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchCodingAgentCommands: vi.fn(() => Promise.resolve(response)),
}));
vi.mock('../../../store/actions/chat-claude-code', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/chat-claude-code')>()),
  sendCodingAgentControl,
}));

const { CodingAgentControlMenu } = await import('../CodingAgentControlMenu');

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

describe('a pick in the coding-agent menu', () => {
  it('frees the UI behind before its control request answers', async () => {
    render(<CodingAgentControlMenu threadId="t1" codingAgent="claude-code" />, host);
    await settle();
    click('.commands-btn-active');
    await settle();
    click('.control-item', 'Permission mode');
    await settle();
    click('.control-option[data-value="plan"]');
    await settle();

    expect(sendCodingAgentControl).toHaveBeenCalledTimes(1);
    expect(answerControl, 'the control request already answered').not.toBeNull();
    expect(document.documentElement.hasAttribute('data-overlay-open'),
      'the menu still holds the UI behind inert').toBe(false);

    answerControl!('ok');
    await settle();
  });
});
