// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

import type { McpServersResponse, McpToolStatus } from '../../../api/client';

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchMcpServers: vi.fn(),
  setMcpToolDisabled: vi.fn(),
  getMcpAllowedTools: vi.fn().mockResolvedValue(''),
}));

const client = await import('../../../api/client');
const { McpServersPage } = await import('../McpServersPage');
const { mcpServersVersion } = await import('../../../store/store');

type Mock = ReturnType<typeof vi.fn>;

function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

function settled(): Promise<void> {
  return act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

function tool(name: string, disabled: boolean): McpToolStatus {
  return {
    name, wire_name: `mcp__slack__${name}`, description: '', disabled, chars: 100, tokens: 40,
  };
}

function served(off: string[] = []): McpServersResponse {
  return {
    servers: [{
      id: 'slack', name: 'Slack', running: true, auto_approve: false, dispatchable: true,
      launch_line: null, launch_approved: false,
      tools_source: 'live', tools_observed_at: null,
      tools: ['a', 'b'].map((n) => tool(n, off.includes(n))),
      chars: 200, tokens: 80, disabled_chars: 0, disabled_tokens: 0,
    }],
    totals: {
      servers: 1, running_servers: 1, tools: 2, chars: 200, tokens: 80, stopped_tools: 0,
      stopped_chars: 0, stopped_tokens: 0, disabled_tools: 0, disabled_chars: 0, disabled_tokens: 0,
    },
    model: 'claude-sonnet-5',
    context_window: 200000,
    budget: {
      request_chars: 288000, mcp_ceiling_chars: 72000, sent_tools: 2, sent_chars: 200,
      sent_tokens: 80, dropped: [], warning: null,
    },
  } as unknown as McpServersResponse;
}

describe('MCP tool switches', () => {
  let host: HTMLElement;
  const toggle = (name: string) =>
    host.querySelector<HTMLInputElement>(`input[aria-label="Offer mcp__slack__${name} to the agent"]`)!;
  async function flip(name: string, offered: boolean) {
    await act(() => {
      toggle(name).checked = offered;
      toggle(name).dispatchEvent(new Event('change', { bubbles: true }));
    });
  }
  const switched = () => (client.setMcpToolDisabled as Mock).mock.calls
    .map(([, wire, disabled]) => `${String(wire).replace('mcp__slack__', '')}:${disabled ? 'off' : 'on'}`);

  beforeEach(async () => {
    vi.clearAllMocks();
    host = document.createElement('div');
    document.body.appendChild(host);
    (client.fetchMcpServers as Mock).mockResolvedValueOnce(served());
    await act(() => render(<McpServersPage />, host));
    await settled();
    await act(() => host.querySelector<HTMLButtonElement>('.settings-disclosure-toggle')!.click());
    await settled();
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('a reload served before the write committed does not bounce the switch', async () => {
    (client.setMcpToolDisabled as Mock).mockReturnValue(new Promise(() => {}));
    const reload = deferred<McpServersResponse>();
    (client.fetchMcpServers as Mock).mockReturnValueOnce(reload.promise);

    await flip('a', false);
    await act(() => { mcpServersVersion.value++; });
    reload.resolve(served());
    await settled();

    expect(toggle('a').checked).toBe(false);
  });

  it('a second switch made while the first is out reaches the engine', async () => {
    const first = deferred<{ disabled_tools: string[] }>();
    (client.setMcpToolDisabled as Mock)
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({ disabled_tools: ['mcp__slack__a', 'mcp__slack__b'] });
    const reload = deferred<McpServersResponse>();
    (client.fetchMcpServers as Mock).mockReturnValueOnce(reload.promise).mockResolvedValue(served(['a', 'b']));

    await flip('a', false);
    await flip('b', false);
    await act(() => { mcpServersVersion.value++; });
    reload.resolve(served());
    await settled();
    expect([toggle('a').checked, toggle('b').checked]).toEqual([false, false]);

    first.resolve({ disabled_tools: ['mcp__slack__a'] });
    await settled();

    expect(switched()).toEqual(['a:off', 'b:off']);
  });

  it('sends one tool a switch at a time, and its latest once the first lands', async () => {
    const first = deferred<{ disabled_tools: string[] }>();
    (client.setMcpToolDisabled as Mock)
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({ disabled_tools: [] });

    await flip('a', false);
    await flip('a', true);
    expect(switched()).toEqual(['a:off']);

    first.resolve({ disabled_tools: ['mcp__slack__a'] });
    await settled();

    expect(switched()).toEqual(['a:off', 'a:on']);
  });

  it('a refused switch shows the engine set again and says why', async () => {
    (client.setMcpToolDisabled as Mock).mockRejectedValueOnce(new Error('request timed out'));
    (client.fetchMcpServers as Mock).mockResolvedValue(served());

    await flip('a', false);
    await settled();

    expect(toggle('a').checked).toBe(true);
  });
});

describe('MCP servers page loads', () => {
  it('a first fetch failing after a newer one served keeps the list', async () => {
    vi.clearAllMocks();
    const host = document.createElement('div');
    document.body.appendChild(host);
    let rejectFirst!: (e: unknown) => void;
    (client.fetchMcpServers as Mock)
      .mockReturnValueOnce(new Promise((_, reject) => { rejectFirst = reject; }))
      .mockResolvedValueOnce(served());

    await act(() => render(<McpServersPage />, host));
    await act(() => { mcpServersVersion.value++; });
    await settled();
    rejectFirst(new Error('request timed out'));
    await settled();

    expect(host.querySelector('.settings-disclosure-toggle')).not.toBeNull();
    render(null, host);
    host.remove();
  });
});
