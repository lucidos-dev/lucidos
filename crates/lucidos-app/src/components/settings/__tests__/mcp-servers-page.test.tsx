import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { mcpServerRow, mcpServersBody, type McpServersBodyProps } from '../McpServersPage';
import { intentsAfterServe, toolIntentKey, withToolIntents } from '../mcpToolIntents';
import {
  describeMcpRowError,
  mcpHeaderSummary,
  mcpServerCostLine,
  mcpServerStateLabel,
  patchToolDisabled,
  sortToolsByCost,
} from '../mcpCost';
import type {
  McpBudget,
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

/** The engine's figures for a 200k window: 288k request chars, a quarter of
 *  it for MCP. */
function budget(over: Partial<McpBudget> = {}): McpBudget {
  return {
    request_chars: 288000,
    mcp_ceiling_chars: 72000,
    sent_tools: 1,
    sent_chars: 500,
    sent_tokens: 200,
    dropped: [],
    warning: null,
    ...over,
  };
}

function response(over: Partial<McpServersResponse> = {}): McpServersResponse {
  return {
    servers: [server()],
    totals: totals(),
    model: 'claude-sonnet-5',
    context_window: 200000,
    budget: budget(),
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
    expect(text).toContain('Up to 1 tool, ~200 tokens per request, in a thread that enables every server');
    expect(text).not.toContain('empty-state');
  });

  it('says once that a running server only runs for this session', () => {
    const text = body({ status: 'loaded', data: response() });
    expect(text).toContain('Nothing starts an MCP server when the engine starts');
    expect(text).toContain("A thread's next turn starts");
  });
});

describe('the skeleton row is the real row', () => {
  it('draws the same structure with no data in it', () => {
    const text = vnodeToText(mcpServerRow({ sk: true }));
    for (const cls of ['mcp-server-block', 'mcp-server-heading', 'label label-neutral', 'mcp-server-id', 'mcp-server-cost', 'mcp-switch', 'action-btn-danger']) {
      expect(text, `the skeleton row dropped ${cls}`).toContain(cls);
    }
    expect(text).not.toContain('Slack');
    expect(text).not.toContain('tokens');
  });
});

describe('header cost figures', () => {
  it('reports the worst case, every server enabled in one thread, and its share of the request budget', () => {
    const summary = mcpHeaderSummary(
      totals({ running_servers: 2 }),
      200000,
      'claude-sonnet-5',
      budget({ sent_tools: 23, sent_chars: 72000, sent_tokens: 28800 }),
    );
    expect(summary.live).toBe('Up to 23 tools, ~29k tokens per request, in a thread that enables every server');
    // Chars over the request's char budget: a quarter of it is 25%, where
    // tokens over the raw window would have said 14%.
    expect(summary.share).toBe('25% of what a request to claude-sonnet-5 can carry (200k context window)');
  });

  it('never reports a real cost as 0% of the budget', () => {
    const summary = mcpHeaderSummary(totals(), 200000, 'claude-sonnet-5', budget({ sent_chars: 300 }));
    expect(summary.share).toBe('Under 1% of what a request to claude-sonnet-5 can carry (200k context window)');
  });

  it('states a 1M window as 1M, matching the marker the id carries', () => {
    const summary = mcpHeaderSummary(
      totals(),
      1_000_000,
      'claude-opus-5[1m]',
      budget({ request_chars: 1_488_000, sent_chars: 0 }),
    );
    expect(summary.share).toBe('0% of what a request to claude-opus-5[1m] can carry (1M context window)');
  });

  it('omits the share when the engine reports no budget', () => {
    expect(mcpHeaderSummary(totals(), 0, 'mystery-model', budget({ request_chars: 0 })).share).toBeNull();
  });

  it("shows the engine's warning as it was written, and nothing when there is none", () => {
    const warning = 'A thread that enables every server leaves 3 tools out.';
    expect(mcpHeaderSummary(totals(), 200000, 'm', budget({ warning })).warning).toBe(warning);
    expect(mcpHeaderSummary(totals(), 200000, 'm', budget()).warning).toBeNull();
  });

  it('renders the warning line in the header once the engine sends one', () => {
    const warning = 'A thread that enables every server leaves 3 tools out.';
    const text = body({ status: 'loaded', data: response({ budget: budget({ warning }) }) });
    expect(text).toContain('mcp-cost-warning');
    expect(text).toContain(warning);
    expect(body({ status: 'loaded', data: response() })).not.toContain('mcp-cost-warning');
  });

  it('says how many servers run, and that a thread pays only for the ones it enabled', () => {
    const summary = mcpHeaderSummary(
      totals({ servers: 3, running_servers: 1, stopped_tools: 40, stopped_tokens: 12000 }),
      200000,
      'claude-sonnet-5',
      budget({ sent_tools: 43, sent_tokens: 12900 }),
    );
    expect(summary.live).toBe('Up to 43 tools, ~13k tokens per request, in a thread that enables every server');
    expect(summary.running).toBe(
      '1 of 3 servers running this session. A thread pays only for the servers it enabled',
    );
  });

  it('states the disabled subtotal so the per-tool switch visibly pays', () => {
    const summary = mcpHeaderSummary(totals({ disabled_tools: 3, disabled_tokens: 800 }), 200000, 'm', budget());
    expect(summary.disabled).toBe('3 tools switched off, keeping ~800 tokens out of every thread');
  });

  it('has nothing to say about servers or off tools when there are none', () => {
    const summary = mcpHeaderSummary(totals({ servers: 0, running_servers: 0 }), 200000, 'm', budget());
    expect(summary.running).toBeNull();
    expect(summary.disabled).toBeNull();
  });

  it('states what a request sends, not the offered total, once tools are dropped', () => {
    const summary = mcpHeaderSummary(
      totals({ running_servers: 2, tools: 500, chars: 750000, tokens: 300000 }),
      200000,
      'claude-sonnet-5',
      budget({ sent_tools: 48, sent_chars: 71000, sent_tokens: 28400 }),
    );
    expect(summary.live).toBe('Up to 48 tools, ~28k tokens per request, in a thread that enables every server');
    expect(summary.share).toBe('25% of what a request to claude-sonnet-5 can carry (200k context window)');
  });

  it('does not claim a cost when no server is registered', () => {
    const summary = mcpHeaderSummary(
      totals({ servers: 0, running_servers: 0, tools: 0, tokens: 0 }),
      200000,
      'm',
      budget(),
    );
    expect(summary.live).toBe('No servers registered, so MCP adds nothing to a request');
  });
});

describe('per-server cost, stated in the tense its state allows', () => {
  it('a running server costs each thread that enables it', () => {
    expect(mcpServerCostLine(server())).toBe('1 tool, ~200 tokens. Adds them to each thread that enables it.');
    expect(mcpServerStateLabel(server())).toBe('Running, this session');
  });

  it('a stopped server states it conditionally and stamps the manifest', () => {
    const line = mcpServerCostLine(server({ running: false, tools_source: 'cache' }));
    expect(line).toContain('Would add 1 tool, ~200 tokens to each thread that enables it.');
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
    expect(mcpServerCostLine(zeroCost)).toContain('Would add 0 tools, ~0 tokens to each thread that enables it.');
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

  it('keeps the tool switches live while a server verb runs', () => {
    // A switch is a pending intent laid over whatever list the verb reloads
    // (ADR 0437), so nothing it does can bounce one back.
    const busy = vnodeToText(mcpServerRow({ server: server(), expanded: true, pending: 'start' }));
    expect(busy).not.toMatch(/disabled="true" ariaLabel="Offer /);
    expect(busy).toContain('ariaLabel="Offer mcp__slack__post_message to the agent"');
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

  it('flips one tool by wire name and leaves the rest alone', () => {
    const before = response({ servers: [server({ tools: [a, b] })] });
    const off = patchToolDisabled(before, 'slack', 'mcp__slack__a', true);
    expect(off.servers[0].tools.map((t) => t.disabled)).toEqual([true, true]);
    const on = patchToolDisabled(off, 'slack', 'mcp__slack__b', false);
    expect(on.servers[0].tools.map((t) => t.disabled)).toEqual([true, false]);
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

describe('pending tool switches', () => {
  const a = tool({ name: 'a', wire_name: 'mcp__slack__a' });
  const list = response({ servers: [server({ tools: [a] })] });
  const key = toolIntentKey('slack', 'mcp__slack__a');
  const off = (ackedAt: number | null) =>
    new Map([[key, { serverId: 'slack', wireName: 'mcp__slack__a', disabled: true, ackedAt }]]);

  it('lays a pending switch over the served list', () => {
    expect(withToolIntents(list, off(null)).servers[0].tools[0].disabled).toBe(true);
  });

  it('keeps a switch over every list fetched before the answer', () => {
    expect(intentsAfterServe(off(null), 9).size).toBe(1);
    expect(intentsAfterServe(off(4), 4).size).toBe(1);
  });

  it('ends a switch once a list fetched after the answer lands, whatever it says', () => {
    expect(intentsAfterServe(off(4), 5).size).toBe(0);
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

describe('the page keeps no write fences', () => {
  // Pending switches end on the engine's acknowledgement (ADR 0437). The write
  // queue and the counters that guessed at it are gone, and must stay gone.
  const here = dirname(fileURLToPath(import.meta.url));
  const src: string = readFileSync(resolve(here, '../McpServersPage.tsx'), 'utf8');

  it('holds no write counter, write queue or reload predicate', () => {
    for (const name of ['localWrites', 'stillWanted', 'isLast', 'writes.current']) {
      expect(src, name).not.toContain(name);
    }
  });
});
