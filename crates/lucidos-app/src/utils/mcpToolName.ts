/** The server and tool halves of an `mcp__<server>__<tool>` name, or `null`
 *  for any other name. Both halves are non-empty, and the tool half keeps any
 *  later `__`. Twin of Rust `McpManager::parse_mcp_tool_name`. */
export function parseMcpToolName(name: string): { server: string; tool: string } | null {
  const match = /^mcp__(.+?)__(.+)$/.exec(name);
  return match ? { server: match[1], tool: match[2] } : null;
}
