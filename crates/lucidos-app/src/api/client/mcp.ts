import { API, json, mutatingFetch, throwIfNotOk } from './_core';

/** Where the tool list in an {@link McpServerStatus} came from. Mirrors the
 *  engine's `McpToolsSource` (kebab-case wire values).
 *
 *  `cache` and `never-observed` are deliberately distinct. A server nobody has
 *  connected to has an empty manifest, and reporting that as a zero-cost server
 *  states something the engine does not know. */
export type McpToolsSource = 'live' | 'cache' | 'never-observed';

/** One tool of one server, with what it costs the request that carries it. */
export interface McpToolStatus {
  /** The server's own spelling, which is not always what the model is shown. */
  name: string;
  /** The name the model is offered. `null` when no usable one exists, so the
   *  tool can never be called and costs nothing. */
  wire_name: string | null;
  description: string | null;
  /** Switched off by the user, so it is absent from every request. */
  disabled: boolean;
  chars: number;
  tokens: number;
}

/** One registered MCP server, with what its ENABLED tools cost. That figure
 *  stands whether or not the server is up: for a stopped one it answers "what
 *  would this cost if I switched it on". */
export interface McpServerStatus {
  id: string;
  name: string;
  running: boolean;
  auto_approve: boolean;
  /** False when the stored id cannot ride a wire tool name, so no tool on this
   *  server can ever be called. Starting it is refused, and Remove is the only
   *  thing to offer. */
  dispatchable: boolean;
  tools_source: McpToolsSource;
  /** ISO timestamp of the last successful connect, or `null` if there has
   *  never been one. */
  tools_observed_at: string | null;
  tools: McpToolStatus[];
  chars: number;
  tokens: number;
  /** What the switched-off tools would add back. */
  disabled_chars: number;
  disabled_tokens: number;
}

/** What the registered servers cost a thread that enables them, split by
 *  whether the process runs (ADR 0430). Every figure is computed in the engine
 *  through the same helpers the request packer uses: format these, never
 *  derive them. */
export interface McpCostTotals {
  servers: number;
  running_servers: number;
  /** What the running servers offer a thread that enables them. */
  tools: number;
  chars: number;
  tokens: number;
  /** What the stopped servers would add to a thread that enabled them.
   *  Excludes servers whose id cannot dispatch, since those can never run. */
  stopped_tools: number;
  stopped_chars: number;
  stopped_tokens: number;
  /** What the switched-off tools would add back, across every server. */
  disabled_tools: number;
  disabled_chars: number;
  disabled_tokens: number;
}

/** The tools of one server the MCP tool ceiling leaves out, by wire name. */
export interface McpDroppedTools {
  server_id: string;
  tools: string[];
}

/** The worst case against the request budget, one thread that enables every
 *  server, in chars, the unit the request packer budgets in. */
export interface McpBudget {
  /** What a whole request may carry: system prompt, tools and messages. */
  request_chars: number;
  /** The part of it the tools of one thread's servers may take together. */
  mcp_ceiling_chars: number;
  /** What that thread's request carries once the ceiling applies. Below the
   *  offered figures exactly when something is dropped. */
  sent_tools: number;
  sent_chars: number;
  sent_tokens: number;
  /** What that thread loses to the ceiling. */
  dropped: McpDroppedTools[];
  /** One line once the tools are past or near the ceiling. */
  warning: string | null;
}

export interface McpServersResponse {
  servers: McpServerStatus[];
  totals: McpCostTotals;
  /** The resolved chat model, which is whose window `context_window` is. */
  model: string;
  context_window: number;
  budget: McpBudget;
}

export function fetchMcpServers(): Promise<McpServersResponse> {
  return json(`${API}/mcp/servers`);
}

/** Start a registered server. Three distinct rejections, each an `ApiError`:
 *  422 for an id that cannot ride the wire, 502 for a process that failed to
 *  start, 404 for an id no longer registered. */
export async function startMcpServer(id: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/mcp/servers/${encodeURIComponent(id)}/start`, {
    method: 'POST',
  });
  await throwIfNotOk(resp);
}

/** Stop a running server. Stopping one that is already stopped succeeds: the
 *  caller asked for a state and got it. */
export async function stopMcpServer(id: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/mcp/servers/${encodeURIComponent(id)}/stop`, {
    method: 'POST',
  });
  await throwIfNotOk(resp);
}

/** Trust every tool on this server, so the agent never shows a permission card
 *  for one. Keeps its body form, unlike the per-server verbs around it, because
 *  the chat permission card already calls it that way. */
export async function setMcpAutoApprove(id: string, autoApprove: boolean): Promise<void> {
  const resp = await mutatingFetch(`${API}/mcp/auto-approve`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ server_id: id, auto_approve: autoApprove }),
  });
  await throwIfNotOk(resp);
}

/** Remove a server, stopping it first. 404 for an unknown id. */
export async function removeMcpServer(id: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/mcp/servers/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
  await throwIfNotOk(resp);
}

/** Switch one tool off or back on, by WIRE name. One tool per call, so two
 *  clients' switches commute (ADR 0437). Answers with the stored set. */
export async function setMcpToolDisabled(
  id: string,
  wireName: string,
  disabled: boolean,
): Promise<{ disabled_tools: string[] }> {
  const resp = await mutatingFetch(
    `${API}/mcp/servers/${encodeURIComponent(id)}/disabled-tools`,
    {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ wire_name: wireName, disabled }),
    },
  );
  await throwIfNotOk(resp);
  return resp.json();
}

// --- Chat MCP permission allowlist (<workspace>/.lucidos/mcp-allowed-tools) ---
export async function getMcpAllowedTools(): Promise<string> {
  const body = await json<{ contents: string }>(`${API}/mcp-allowed-tools`);
  return body.contents;
}

export async function putMcpAllowedTools(contents: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/mcp-allowed-tools`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ contents }),
  });
  await throwIfNotOk(resp);
}
