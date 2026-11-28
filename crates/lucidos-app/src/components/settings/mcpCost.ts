import type {
  McpBudget,
  McpCostTotals,
  McpServerStatus,
  McpServersResponse,
  McpToolStatus,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { contextPercent, formatTokens } from '../../utils/formatTokens';
import { formatTimeAgo } from '../../utils/formatTime';


function plural(n: number, word: string): string {
  return n === 1 ? `1 ${word}` : `${n} ${word}s`;
}

export function callableToolCount(server: McpServerStatus): number {
  return server.tools.filter((t) => !t.disabled && t.wire_name !== null).length;
}

export type McpServerState = 'running' | 'stopped' | 'undispatchable';

export function mcpServerState(server: McpServerStatus): McpServerState {
  if (server.running) return 'running';
  return server.dispatchable ? 'stopped' : 'undispatchable';
}

export function mcpServerStateLabel(server: McpServerStatus): string {
  switch (mcpServerState(server)) {
    case 'running':
      return 'Running, this session';
    case 'stopped':
      return 'Stopped';
    case 'undispatchable':
      return 'Unusable id';
  }
}

function observedStamp(server: McpServerStatus): string | null {
  if (!server.tools_observed_at) return null;
  return `Tools last seen ${formatTimeAgo(new Date(server.tools_observed_at))}.`;
}

export function mcpServerCostLine(server: McpServerStatus): string {
  if (server.tools_source === 'never-observed') {
    return 'Tools never observed, so what this server would cost is unknown. Start it once to find out.';
  }
  const cost = `${plural(callableToolCount(server), 'tool')}, ~${formatTokens(server.tokens)} tokens`;
  if (server.running) return `${cost}. Adds them to each thread that enables it.`;
  const seen = observedStamp(server);
  const tail = seen ? ` ${seen}` : '';
  if (!server.dispatchable) {
    return `${cost} cached, and none of them can ever be called.${tail}`;
  }
  return `Would add ${cost} to each thread that enables it.${tail}`;
}

export function mcpToolCostLine(tool: McpToolStatus): string {
  if (tool.wire_name === null) {
    return 'No usable name on the wire, so it can never be called and costs nothing.';
  }
  if (tool.disabled) {
    return `Switched off, keeping ~${formatTokens(tool.tokens)} tokens out of every thread.`;
  }
  return `~${formatTokens(tool.tokens)} tokens in each thread that enables the server.`;
}

export function sortToolsByCost(tools: McpToolStatus[]): McpToolStatus[] {
  return [...tools].sort(
    (a, b) => b.tokens - a.tokens || (a.wire_name ?? a.name).localeCompare(b.wire_name ?? b.name),
  );
}

export function patchToolDisabled(
  data: McpServersResponse,
  serverId: string,
  wireName: string,
  disabled: boolean,
): McpServersResponse {
  return {
    ...data,
    servers: data.servers.map((server) =>
      server.id !== serverId
        ? server
        : {
            ...server,
            tools: server.tools.map((tool) =>
              tool.wire_name === wireName ? { ...tool, disabled } : tool,
            ),
          },
    ),
  };
}

export interface McpHeaderSummary {
  live: string;
  share: string | null;
  warning: string | null;
  running: string | null;
  disabled: string | null;
}

export function mcpHeaderSummary(
  totals: McpCostTotals,
  contextWindow: number,
  model: string,
  budget: McpBudget,
): McpHeaderSummary {
  const live =
    totals.servers === 0
      ? 'No servers registered, so MCP adds nothing to a request'
      : `Up to ${plural(budget.sent_tools, 'tool')}, ~${formatTokens(budget.sent_tokens)} tokens per request, in a thread that enables every server`;

  let share: string | null = null;
  if (budget.request_chars > 0) {
    const pct = contextPercent(budget.sent_chars, budget.request_chars);
    const text = pct === 0 && budget.sent_chars > 0 ? 'Under 1%' : `${pct}%`;
    share = `${text} of what a request to ${model} can carry (${formatTokens(contextWindow)} context window)`;
  }

  const running =
    totals.servers === 0
      ? null
      : `${totals.running_servers} of ${plural(totals.servers, 'server')} running this session. A thread pays only for the servers it enabled`;

  const disabled =
    totals.disabled_tools === 0
      ? null
      : `${plural(totals.disabled_tools, 'tool')} switched off, keeping ~${formatTokens(totals.disabled_tokens)} tokens out of every thread`;

  return { live, share, warning: budget.warning, running, disabled };
}

export type McpRowErrorKind = 'unusable-id' | 'start-failed' | 'gone' | 'other';

export interface McpRowError {
  kind: McpRowErrorKind;
  message: string;
}

function httpCodeOf(error: unknown): number | undefined {
  if (error instanceof Error && 'httpCode' in error) {
    const code = (error as { httpCode: unknown }).httpCode;
    if (typeof code === 'number') return code;
  }
  return undefined;
}

function reasonOf(error: unknown): string {
  if (error instanceof Error && 'reason' in error) {
    const reason = (error as { reason: unknown }).reason;
    if (typeof reason === 'string' && reason.length > 0) return reason;
  }
  return errorDetail(error);
}

export function describeMcpRowError(error: unknown): McpRowError {
  const message = reasonOf(error);
  switch (httpCodeOf(error)) {
    case 404:
      return { kind: 'gone', message };
    case 422:
      return { kind: 'unusable-id', message };
    case 502:
      return { kind: 'start-failed', message };
    default:
      return { kind: 'other', message };
  }
}
