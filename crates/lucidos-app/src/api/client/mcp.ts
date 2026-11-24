import { API, json, mutatingFetch, throwIfNotOk } from './_core';

export type McpToolsSource = 'live' | 'cache' | 'never-observed';

export interface McpToolStatus {
  name: string;
  wire_name: string | null;
  description: string | null;
  disabled: boolean;
  chars: number;
  tokens: number;
}

export interface McpServerStatus {
  id: string;
  name: string;
  running: boolean;
  auto_approve: boolean;
  dispatchable: boolean;
  launch_line: string | null;
  launch_approved: boolean;
  tools_source: McpToolsSource;
  tools_observed_at: string | null;
  tools: McpToolStatus[];
  chars: number;
  tokens: number;
  disabled_chars: number;
  disabled_tokens: number;
}

export interface McpCostTotals {
  servers: number;
  running_servers: number;
  tools: number;
  chars: number;
  tokens: number;
  stopped_tools: number;
  stopped_chars: number;
  stopped_tokens: number;
  disabled_tools: number;
  disabled_chars: number;
  disabled_tokens: number;
}

export interface McpDroppedTools {
  server_id: string;
  tools: string[];
}

export interface McpBudget {
  request_chars: number;
  mcp_ceiling_chars: number;
  sent_tools: number;
  sent_chars: number;
  sent_tokens: number;
  dropped: McpDroppedTools[];
  warning: string | null;
}

export interface McpServersResponse {
  servers: McpServerStatus[];
  totals: McpCostTotals;
  model: string;
  context_window: number;
  budget: McpBudget;
}

export function fetchMcpServers(): Promise<McpServersResponse> {
  return json(`${API}/mcp/servers`);
}

export async function startMcpServer(id: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/mcp/servers/${encodeURIComponent(id)}/start`, {
    method: 'POST',
  });
  await throwIfNotOk(resp);
}

export async function stopMcpServer(id: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/mcp/servers/${encodeURIComponent(id)}/stop`, {
    method: 'POST',
  });
  await throwIfNotOk(resp);
}

export async function setMcpAutoApprove(id: string, autoApprove: boolean): Promise<void> {
  const resp = await mutatingFetch(`${API}/mcp/auto-approve`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ server_id: id, auto_approve: autoApprove }),
  });
  await throwIfNotOk(resp);
}

export async function removeMcpServer(id: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/mcp/servers/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
  await throwIfNotOk(resp);
}

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
