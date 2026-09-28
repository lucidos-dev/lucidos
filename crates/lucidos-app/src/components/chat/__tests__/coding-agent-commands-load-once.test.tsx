// @vitest-environment jsdom
/**
 * One commands read per cue, never a loop. `loadCommands` writes the module
 * command cache when its response lands, so an effect that calls it must not
 * also subscribe to that cache. When it did, every response re-ran the effect:
 * the compose view wiped its commands and fetched again, forever, and the
 * button sat dimmed as if it had no commands.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import type { CodingAgentCommandsResponse } from '../../../api/client';

const response: CodingAgentCommandsResponse = {
  control_commands: [{ subtype: 'set_model', label: 'Model', params: [] }],
  builtin_commands: ['clear'],
  skill_commands: [],
  has_active_session: true,
} as unknown as CodingAgentCommandsResponse;

const fetchCodingAgentCommands = vi.fn(() => Promise.resolve(response));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchCodingAgentCommands,
}));

const { CodingAgentControlMenu } = await import('../CodingAgentControlMenu');
const { codingAgentSessionVersion } = await import('../../../store/store');

let host: HTMLDivElement;

/** Enough macrotasks for the first effect to run and several responses to land. */
async function settle(): Promise<void> {
  for (let i = 0; i < 80; i++) await new Promise(r => setTimeout(r, 0));
}

beforeEach(() => {
  fetchCodingAgentCommands.mockClear();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  codingAgentSessionVersion.value = 0;
});

describe('the coding-agent commands read', () => {
  it('runs once in the compose view, and the button is ready', async () => {
    render(<CodingAgentControlMenu codingAgent="claude-code" />, host);
    await settle();

    expect(fetchCodingAgentCommands).toHaveBeenCalledTimes(1);
    expect(host.querySelector('.commands-btn-active')).not.toBeNull();
  });

  it('runs once per session cue on an active thread', async () => {
    codingAgentSessionVersion.value = 1;
    render(<CodingAgentControlMenu threadId="t1" codingAgent="claude-code" />, host);
    await settle();
    const afterMount = fetchCodingAgentCommands.mock.calls.length;
    // The first test left the cache loaded, so an active button alone proves
    // nothing about this mount.
    expect(afterMount).toBeGreaterThan(0);

    await settle();
    expect(fetchCodingAgentCommands.mock.calls.length).toBe(afterMount);
  });
});
