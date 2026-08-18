import type {
  McpCostTotals,
  McpServerStatus,
  McpServersResponse,
  McpToolStatus,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { contextPercent, formatTokens } from '../../utils/formatTokens';
import { formatTimeAgo } from '../../utils/formatTime';

/* Every char and token figure on the MCP Servers page is a field of the API
   response. This module FORMATS those fields and never derives one: there is
   deliberately no chars-to-tokens arithmetic here, and adding some is how the
   Context Viewer once reported a measured 205k prompt as 361k. See the standing
   ban in `utils/formatTokens.ts`. */

function plural(n: number, word: string): string {
  return n === 1 ? `1 ${word}` : `${n} ${word}s`;
}

/** Tools the model is actually offered: enabled, and carrying a wire name. A
 *  tool with no wire name can never be called, so counting it would overstate
 *  what the server gives. */
export function callableToolCount(server: McpServerStatus): number {
  return server.tools.filter((t) => !t.disabled && t.wire_name !== null).length;
}

/** What the page shows this server as. Mirrors the engine's own split in
 *  `McpCostTotals::of`: a stopped server whose id cannot ride the wire is
 *  neither running nor startable, so it is its own state. */
export type McpServerState = 'running' | 'stopped' | 'undispatchable';

export function mcpServerState(server: McpServerStatus): McpServerState {
  if (server.running) return 'running';
  return server.dispatchable ? 'stopped' : 'undispatchable';
}

/** "Running, this session" and never a bare "Running".
 *
 *  Nothing starts an MCP server at boot, so the state really does reset on
 *  restart. A label implying otherwise is a lie the user only finds out about
 *  after the next restart. */
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

/** When the cached manifest was taken, so a week-old observation never reads as
 *  live. `null` for a server whose manifest has never been observed, which the
 *  cost line states in its own words instead. */
function observedStamp(server: McpServerStatus): string | null {
  if (!server.tools_observed_at) return null;
  return `Tools last seen ${formatTimeAgo(new Date(server.tools_observed_at))}.`;
}

/** What this server costs, stated in the tense its state allows.
 *
 *  A running server's figure is what every request is paying now. A stopped
 *  one's is conditional and carries the manifest date. A never-observed one has
 *  no figure at all, which is the distinction the manifest cache exists for: a
 *  server nobody has connected to is unknown, not free. */
export function mcpServerCostLine(server: McpServerStatus): string {
  if (server.tools_source === 'never-observed') {
    return 'Tools never observed, so what this server would cost is unknown. Start it once to find out.';
  }
  const cost = `${plural(callableToolCount(server), 'tool')}, ~${formatTokens(server.tokens)} tokens`;
  if (server.running) return `${cost} in every request.`;
  const seen = observedStamp(server);
  const tail = seen ? ` ${seen}` : '';
  if (!server.dispatchable) {
    return `${cost} cached, and none of them can ever be called.${tail}`;
  }
  return `Would add ${cost} if switched on.${tail}`;
}

/** What one tool costs the request that carries it. */
export function mcpToolCostLine(tool: McpToolStatus): string {
  if (tool.wire_name === null) {
    return 'No usable name on the wire, so it can never be called and costs nothing.';
  }
  if (tool.disabled) {
    return `Switched off, keeping ~${formatTokens(tool.tokens)} tokens out of every request.`;
  }
  return `~${formatTokens(tool.tokens)} tokens in every request.`;
}

/** Tools by descending cost, because the expensive one is what the user came
 *  for. Ties fall back to the wire name so the order is stable across reloads. */
export function sortToolsByCost(tools: McpToolStatus[]): McpToolStatus[] {
  return [...tools].sort(
    (a, b) => b.tokens - a.tokens || (a.wire_name ?? a.name).localeCompare(b.wire_name ?? b.name),
  );
}

/** The wire names this server has switched off, which is exactly the set the
 *  PUT replaces.
 *
 *  Read off the server the page is SHOWING, after the optimistic flip. That is
 *  what makes two quick clicks safe: the second write carries the first one's
 *  flip too, so neither can undo the other. A name left over from a tool the
 *  server no longer ships also drops out, which is right: it would filter
 *  nothing anyway. */
export function disabledWireNames(server: McpServerStatus): string[] {
  return server.tools
    .filter((t) => t.disabled && t.wire_name !== null)
    .map((t) => t.wire_name as string);
}

/** Flip one tool's switch in the loaded response, and nothing else.
 *
 *  The optimistic half of a per-tool disable: the switch moves at once, while
 *  every char and token figure waits for the engine's answer. Recomputing a
 *  total here would need a ratio the frontend must not own. */
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

/** The header lines, each `null` when it has nothing to say. */
export interface McpHeaderSummary {
  /** What every request is paying right now. */
  live: string;
  /** Share of the resolved model's window, or `null` when the engine reports
   *  no window for it. */
  share: string | null;
  /** What the stopped servers would add if switched on. */
  stopped: string | null;
  /** What the switched-off tools are keeping out. */
  disabled: string | null;
}

export function mcpHeaderSummary(
  totals: McpCostTotals,
  contextWindow: number,
  model: string,
): McpHeaderSummary {
  const live =
    totals.running_servers === 0
      ? 'No servers on, so MCP adds nothing to a request'
      : `${plural(totals.running_servers, 'server')} on, ${plural(totals.tools, 'tool')}, ~${formatTokens(totals.tokens)} tokens per request`;

  let share: string | null = null;
  if (contextWindow > 0) {
    const pct = contextPercent(totals.tokens, contextWindow);
    // A figure that rounds to zero is still a cost, and "0%" reads as free.
    const text = pct === 0 && totals.tokens > 0 ? 'Under 1%' : `${pct}%`;
    share = `${text} of ${model}'s ${formatTokens(contextWindow)} context window`;
  }

  const stopped =
    totals.stopped_tools === 0
      ? null
      : `${plural(totals.stopped_tools, 'more tool')} available, ~${formatTokens(totals.stopped_tokens)} tokens if switched on`;

  const disabled =
    totals.disabled_tools === 0
      ? null
      : `${plural(totals.disabled_tools, 'tool')} switched off, keeping ~${formatTokens(totals.disabled_tokens)} tokens out of every request`;

  return { live, share, stopped, disabled };
}

/** How a failed per-server verb reads on the row.
 *
 *  `unusable-id` is permanent and Remove is the only way out. `start-failed`
 *  carries the process error and is worth retrying once the cause is fixed.
 *  `gone` means the row no longer exists, so the page reloads the list rather
 *  than annotating something it is about to drop. */
export type McpRowErrorKind = 'unusable-id' | 'start-failed' | 'gone' | 'other';

export interface McpRowError {
  kind: McpRowErrorKind;
  message: string;
}

/** The HTTP status an `ApiError` carries, if this is one. Duck-typed rather
 *  than an `instanceof` so this module stays clear of the API client's own
 *  import graph, matching `toFailed` in `store/types.ts`. */
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
