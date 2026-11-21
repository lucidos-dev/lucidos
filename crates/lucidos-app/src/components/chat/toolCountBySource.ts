import { parseMcpToolName } from '../../utils/mcpToolName';

/** Where a turn's tools came from, for the context viewer's meta line.
 *
 *  An MCP tool is named `mcp__<server>__<tool>`. Every other tool is the
 *  producer's own, and `ownLabel` names that group. Since ADR 0430 only the
 *  servers a thread enabled send tools, so the split says what this thread
 *  carried, for example "77 tools: 62 Lucidos, 15 slack". With no MCP tool the
 *  line is the bare count. */
export function toolCountBySource(tools: readonly string[], ownLabel: string): string {
  const servers = new Map<string, number>();
  let own = 0;
  for (const name of tools) {
    const server = parseMcpToolName(name)?.server ?? null;
    if (server === null) own++;
    else servers.set(server, (servers.get(server) ?? 0) + 1);
  }
  const total = tools.length === 1 ? '1 tool' : `${tools.length} tools`;
  if (servers.size === 0) return total;
  const parts = [`${own} ${ownLabel}`, ...[...servers].map(([server, n]) => `${n} ${server}`)];
  return `${total}: ${parts.join(', ')}`;
}
