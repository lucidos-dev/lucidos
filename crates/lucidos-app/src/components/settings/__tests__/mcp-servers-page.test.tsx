import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { mcpServerRow, mcpServersBody, type McpServersBodyProps } from '../McpServersPage';
import {
  describeMcpRowError,
  mcpHeaderSummary,
  mcpServerCostLine,
  mcpServerStateLabel,
  disabledWireNames,
  patchToolDisabled,
  sortToolsByCost,
} from '../mcpCost';
import type {
  McpCostTotals,
  McpServerStatus,
  McpServersResponse,
  McpToolStatus,
} from '../../../api/client';
import type { Loadable } from '../../../store/types';

/** Flatten a vnode tree to a string. Same shallow walk as the helper in
 *  `directory-picker-loadable.test.tsx`, with two differences the row needs.
 *
 *  A COMPONENT vnode keeps its tag and its scalar props. The row draws most of
 *  its text through `<SkText class=...>`. It also hands each switch its
 *  accessible name as a prop, so dropping either would hide what is asserted.
 *
 *  Only `children` is recursed into. A vnode merely PASSED as a prop, such as a
 *  `<LoadingFade>` skeleton, therefore does not count as rendered: it is
 *  constructed on every render and drawn on almost none. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<Record<string, unknown>>;
  const props = (v.props ?? {}) as Record<string, unknown>;
  const scalar = (value: unknown) =>
    typeof value === 'string' || typeof value === 'number' || value === true;
  const attrs = Object.entries(props)
    .filter(([k, value]) => k !== 'children' && scalar(value))
    .map(([k, value]) => ` ${k}="${String(value)}"`)
    .join('');
  const tag = typeof v.type === 'string' ? v.type : ((v.type as { name?: string })?.name ?? 'C');
  return `<${tag}${attrs}>${vnodeToText(props.children as ComponentChildren)}</${tag}>`;
}

function tool(over: Partial<McpToolStatus> = {}): McpToolStatus {
  return {
    name: 'post_message',
    wire_name: 'mcp__slack__post_message',
    description: 'Post a message to a channel',
    disabled: false,
    chars: 500,
    tokens: 200,
    ...over,
  };
}

function server(over: Partial<McpServerStatus> = {}): McpServerStatus {
  return {
    id: 'slack',
    name: 'Slack',
    running: true,
    auto_approve: false,
    dispatchable: true,
    tools_source: 'live',
    tools_observed_at: new Date(Date.now() - 3 * 86400000).toISOString(),
    tools: [tool()],
    chars: 500,
    tokens: 200,
    disabled_chars: 0,
    disabled_tokens: 0,
    ...over,
  };
}

function totals(over: Partial<McpCostTotals> = {}): McpCostTotals {
  return {
    servers: 1,
    running_servers: 1,
    tools: 1,
    chars: 500,
    tokens: 200,
    stopped_tools: 0,
    stopped_chars: 0,
    stopped_tokens: 0,
    disabled_tools: 0,
    disabled_chars: 0,
    disabled_tokens: 0,
    ...over,
  };
}

function response(over: Partial<McpServersResponse> = {}): McpServersResponse {
  return {
    servers: [server()],
    totals: totals(),
    model: 'claude-opus-5',
    context_window: 200000,
    ...over,
  };
}

const NOOP = () => {};

function body(data: Loadable<McpServersResponse>, showLoading = false): string {
  const props: McpServersBodyProps = {
    data,
    showLoading,
    expanded: new Set<string>(),
    pending: {},
    errors: {},
    onToggleExpanded: NOOP,
    onSetRunning: NOOP,
    onSetAutoApprove: NOOP,
    onRemove: NOOP,
    onSetToolDisabled: NOOP,
  };
  return vnodeToText(mcpServersBody(props));
}

describe('mcpServersBody (Loadable discipline)', () => {
  it('not-loaded renders the page chrome and no figures, rows or error', () => {
    const text = body({ status: 'not-loaded' });
    expect(text).toContain('Context cost');
    expect(text).toContain('Servers');
    expect(text).not.toContain('showSkeleton="true"');
    expect(text).not.toContain('mcp-cost-headline');
    expect(text).not.toContain('mcp-server-block');
    expect(text).not.toContain('empty-state');
    expect(text).not.toContain('<LoadableError');
  });

  it('raises the skeleton once the delay elapses, and shows no figure', () => {
    const text = body({ status: 'loading' }, true);
    expect(text).toContain('showSkeleton="true"');
    expect(text).not.toContain('tokens per request');
    expect(text).not.toContain('mcp-server-block');
  });

  it('holds the skeleton back before the delay, so a fast load never flashes', () => {
    expect(body({ status: 'loading' }, false)).not.toContain('showSkeleton="true"');
  });

  it('failed renders the error, distinct from empty, and drops the list', () => {
    const text = body({ status: 'failed', error: 'Connection refused', httpCode: 500 });
    expect(text).toContain('<LoadableError');
    expect(text).toContain('noun="MCP servers"');
    expect(text).toContain('error="Connection refused"');
    expect(text).not.toContain('empty-state');
    expect(text).not.toContain('mcp-server-block');
  });

  it('loaded with no servers renders the empty state, not an error', () => {
    const text = body({ status: 'loaded', data: response({ servers: [], totals: totals({ servers: 0, running_servers: 0, tools: 0, chars: 0, tokens: 0 }) }) });
    expect(text).toContain('No MCP servers registered');
    expect(text).not.toContain('Failed to load');
    expect(text).not.toContain('mcp-server-block');
  });

  it('loaded renders a row per server plus the header figures', () => {
    const text = body({ status: 'loaded', data: response() });
    expect(text).toContain('Slack');
    expect(text).toContain('mcp-server-block');
    expect(text).toContain('1 server on, 1 tool, ~200 tokens per request');
    expect(text).not.toContain('empty-state');
  });

  it('says once that a running server only runs for this session', () => {
    const text = body({ status: 'loaded', data: response() });
    expect(text).toContain('Nothing starts an MCP server when the engine starts');
  });
});

describe('the skeleton row is the real row', () => {
  it('draws the same structure with no data in it', () => {
    const text = vnodeToText(mcpServerRow({ sk: true }));
    for (const cls of ['mcp-server-block', 'mcp-server-heading', 'mcp-state-chip', 'mcp-server-id', 'mcp-server-cost', 'mcp-switch', 'action-btn-danger']) {
      expect(text, `the skeleton row dropped ${cls}`).toContain(cls);
    }
    expect(text).not.toContain('Slack');
    expect(text).not.toContain('tokens');
  });
});

describe('header cost figures', () => {
  it('reports servers on, tools, tokens and the share of the window', () => {
    const summary = mcpHeaderSummary(totals({ running_servers: 2, tools: 23, tokens: 4100 }), 200000, 'claude-opus-5');
    expect(summary.live).toBe('2 servers on, 23 tools, ~4k tokens per request');
    expect(summary.share).toBe("2% of claude-opus-5's 200k context window");
  });

  it('never reports a real cost as 0% of the window', () => {
    const summary = mcpHeaderSummary(totals({ tokens: 300 }), 200000, 'claude-opus-5');
    expect(summary.share).toBe("Under 1% of claude-opus-5's 200k context window");
  });

  it('states a 1M window as 1M, matching the marker the id carries', () => {
    const summary = mcpHeaderSummary(totals({ tokens: 0 }), 1_000_000, 'claude-opus-5@default[1m]');
    expect(summary.share).toBe("0% of claude-opus-5@default[1m]'s 1M context window");
  });

  it('omits the share when the engine reports no window', () => {
    expect(mcpHeaderSummary(totals(), 0, 'mystery-model').share).toBeNull();
  });

  it('counts the off servers separately from the per-request total', () => {
    const summary = mcpHeaderSummary(
      totals({ running_servers: 1, tools: 3, tokens: 900, stopped_tools: 40, stopped_tokens: 12000 }),
      200000,
      'claude-opus-5',
    );
    expect(summary.live).toBe('1 server on, 3 tools, ~900 tokens per request');
    expect(summary.stopped).toBe('40 more tools available, ~12k tokens if switched on');
  });

  it('states the disabled subtotal so the per-tool switch visibly pays', () => {
    const summary = mcpHeaderSummary(totals({ disabled_tools: 3, disabled_tokens: 800 }), 200000, 'm');
    expect(summary.disabled).toBe('3 tools switched off, keeping ~800 tokens out of every request');
  });

  it('has nothing to say about off servers or off tools when there are none', () => {
    const summary = mcpHeaderSummary(totals(), 200000, 'm');
    expect(summary.stopped).toBeNull();
    expect(summary.disabled).toBeNull();
  });

  it('does not claim a cost when no server is on', () => {
    const summary = mcpHeaderSummary(totals({ running_servers: 0, tools: 0, tokens: 0 }), 200000, 'm');
    expect(summary.live).toBe('No servers on, so MCP adds nothing to a request');
  });
});

describe('per-server cost, stated in the tense its state allows', () => {
  it('a running server pays now', () => {
    expect(mcpServerCostLine(server())).toBe('1 tool, ~200 tokens in every request.');
    expect(mcpServerStateLabel(server())).toBe('Running, this session');
  });

  it('a stopped server states it conditionally and stamps the manifest', () => {
    const line = mcpServerCostLine(server({ running: false, tools_source: 'cache' }));
    expect(line).toContain('Would add 1 tool, ~200 tokens if switched on.');
    expect(line).toContain('Tools last seen 3d ago.');
    expect(line).not.toContain('in every request');
  });

  it('a never-observed server is not a zero-cost server', () => {
    const unobserved = server({
      running: false,
      tools_source: 'never-observed',
      tools_observed_at: null,
      tools: [],
      chars: 0,
      tokens: 0,
    });
    const zeroCost = server({
      running: false,
      tools_source: 'cache',
      tools: [],
      chars: 0,
      tokens: 0,
    });
    expect(mcpServerCostLine(unobserved)).toBe(
      'Tools never observed, so what this server would cost is unknown. Start it once to find out.',
    );
    expect(mcpServerCostLine(zeroCost)).toContain('Would add 0 tools, ~0 tokens if switched on.');
    expect(mcpServerCostLine(unobserved)).not.toBe(mcpServerCostLine(zeroCost));
  });
});

describe('the undispatchable row', () => {
  const backstage = server({
    id: 'back.stage',
    name: 'Backstage',
    running: false,
    dispatchable: false,
    tools_source: 'cache',
  });

  it('offers Remove and no way to start', () => {
    const text = vnodeToText(mcpServerRow({ server: backstage }));
    expect(text).toContain('Remove');
    expect(text).toContain('data-state="undispatchable"');
    // Both switches are gone: with nothing callable, neither has anything to
    // act on, and a Run toggle would invite a retry that can never work.
    expect(text).not.toContain('mcp-switch');
    expect(text).not.toContain('ariaLabel="Run Backstage"');
  });

  it('says why, where the user is looking', () => {
    const text = vnodeToText(mcpServerRow({ server: backstage }));
    expect(text).toContain("This server's id cannot be used on the wire");
    expect(mcpServerStateLabel(backstage)).toBe('Unusable id');
  });

  it('is distinct from an ordinary stopped row, which does offer Run', () => {
    const text = vnodeToText(mcpServerRow({ server: server({ running: false, tools_source: 'cache' }) }));
    expect(text).toContain('ariaLabel="Run Slack"');
    expect(text).toContain('data-state="stopped"');
  });
});

describe('a row mid-verb and a row that failed', () => {
  it('shows pending rather than flipping the switch', () => {
    const text = vnodeToText(mcpServerRow({ server: server({ running: false, tools_source: 'cache' }), pending: 'start' }));
    expect(text).toContain('Starting...');
    expect(text).toContain('disabled="true"');
  });

  it('holds the tool switches while a server verb runs', () => {
    // The verb reloads the list when it settles, and that reply predates a
    // flip made under it, so an open switch here would bounce back.
    const busy = vnodeToText(mcpServerRow({ server: server(), expanded: true, pending: 'start' }));
    const idle = vnodeToText(mcpServerRow({ server: server(), expanded: true }));
    expect(busy).toMatch(/disabled="true" ariaLabel="Offer /);
    expect(idle).not.toMatch(/disabled="true" ariaLabel="Offer /);
    expect(idle).toContain('ariaLabel="Offer mcp__slack__post_message to the agent"');
  });

  it('renders the failure inline on the row, tagged by kind', () => {
    const text = vnodeToText(
      mcpServerRow({
        server: server({ running: false, tools_source: 'cache' }),
        error: { kind: 'start-failed', message: "MCP server 'slack' failed to start: npx not found" },
      }),
    );
    expect(text).toContain('mcp-row-error');
    expect(text).toContain('data-kind="start-failed"');
    expect(text).toContain('npx not found');
  });
});

describe('describeMcpRowError', () => {
  function apiError(httpCode: number, reason: string): Error {
    return Object.assign(new Error(`${httpCode} ${reason}`), { httpCode, reason });
  }

  it('separates the three route failures the engine distinguishes', () => {
    expect(describeMcpRowError(apiError(422, 'id cannot be used')).kind).toBe('unusable-id');
    expect(describeMcpRowError(apiError(502, 'npx not found')).kind).toBe('start-failed');
    expect(describeMcpRowError(apiError(404, "MCP server 'x' not found")).kind).toBe('gone');
  });

  it('carries the message from the engine, so the user can see what to fix', () => {
    expect(describeMcpRowError(apiError(502, 'npx not found')).message).toBe('npx not found');
  });

  it('falls back for anything that is not an ApiError', () => {
    const described = describeMcpRowError(new Error('Load failed'));
    expect(described.kind).toBe('other');
    expect(described.message).toBe('Load failed');
  });
});

describe('the tool list', () => {
  const cheap = tool({ name: 'a', wire_name: 'mcp__slack__a', tokens: 10 });
  const dear = tool({ name: 'b', wire_name: 'mcp__slack__b', tokens: 900 });
  const middling = tool({ name: 'c', wire_name: 'mcp__slack__c', tokens: 100 });

  it('is ordered by descending cost, because the expensive one is the point', () => {
    const sorted = sortToolsByCost([cheap, dear, middling]);
    expect(sorted.map((t) => t.wire_name)).toEqual([
      'mcp__slack__b',
      'mcp__slack__c',
      'mcp__slack__a',
    ]);
  });

  it('shows the spelling the server uses only where it differs from the wire name', () => {
    const renamed = tool({ name: 'catalog.get-entity', wire_name: 'mcp__x__catalog_get_entity' });
    const expandedRow = (t: McpToolStatus) =>
      vnodeToText(mcpServerRow({ server: server({ tools: [t] }), expanded: true }));
    expect(expandedRow(renamed)).toContain('The server calls it catalog.get-entity');
    expect(expandedRow(tool({ name: 'same', wire_name: 'same' }))).not.toContain('The server calls it');
  });

  it('offers no switch for a tool with no usable wire name', () => {
    const orphan = tool({ name: 'broken', wire_name: null, tokens: 0 });
    const text = vnodeToText(mcpServerRow({ server: server({ tools: [orphan] }), expanded: true }));
    expect(text).toContain('data-state="uncallable"');
    expect(text).toContain('it can never be called and costs nothing');
    expect(text).not.toContain('ariaLabel="Offer');
  });
});

describe('the optimistic per-tool switch', () => {
  const a = tool({ name: 'a', wire_name: 'mcp__slack__a' });
  const b = tool({ name: 'b', wire_name: 'mcp__slack__b', disabled: true });

  it('sends the whole set, by wire name, read off the flipped page', () => {
    const before = response({ servers: [server({ tools: [a, b] })] });
    const off = patchToolDisabled(before, 'slack', 'mcp__slack__a', true);
    expect(disabledWireNames(off.servers[0]).sort()).toEqual([
      'mcp__slack__a',
      'mcp__slack__b',
    ]);
    const on = patchToolDisabled(off, 'slack', 'mcp__slack__b', false);
    expect(disabledWireNames(on.servers[0])).toEqual(['mcp__slack__a']);
  });

  it('carries an earlier flip along, so two quick clicks cannot undo each other', () => {
    // Both writes send the same final set, whichever order their round trips
    // settle in. The bug this pins: a set computed per click sends [A] and
    // [B], and whichever lands last re-enables the other tool.
    const base = response({ servers: [server({ tools: [a, tool({ name: 'b', wire_name: 'mcp__slack__b' })] })] });
    const first = patchToolDisabled(base, 'slack', 'mcp__slack__a', true);
    const second = patchToolDisabled(first, 'slack', 'mcp__slack__b', true);
    expect(disabledWireNames(second.servers[0]).sort()).toEqual([
      'mcp__slack__a',
      'mcp__slack__b',
    ]);
  });

  it('never invents a wire name for a tool that has none', () => {
    const orphan = tool({ name: 'broken', wire_name: null, disabled: true });
    expect(disabledWireNames(server({ tools: [orphan] }))).toEqual([]);
  });

  it('moves the switch and no figure, so no ratio is ever derived here', () => {
    const before = response({ servers: [server({ tools: [a, b] })] });
    const after = patchToolDisabled(before, 'slack', 'mcp__slack__a', true);
    expect(after.servers[0].tools[0].disabled).toBe(true);
    expect(after.servers[0].tokens).toBe(before.servers[0].tokens);
    expect(after.servers[0].disabled_tokens).toBe(before.servers[0].disabled_tokens);
    expect(after.totals).toBe(before.totals);
  });
});

describe('every figure comes from the API', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const SOURCES = ['../McpServersPage.tsx', '../mcpCost.ts'].map((p) =>
    readFileSync(resolve(here, p), 'utf8'),
  );

  it('never turns chars into tokens in the frontend', () => {
    // The Context Viewer once reported a measured 205k prompt as 361k, because
    // a hand-copied chars-per-token ratio lived in TypeScript. `chars` is
    // rendered nowhere and converted nowhere: the page formats `tokens`.
    for (const src of SOURCES) {
      expect(src).not.toMatch(/chars\s*[*/]/);
      expect(src).not.toMatch(/[*/]\s*2\.5/);
      expect(src).not.toMatch(/estimateTokens/);
    }
  });
});
