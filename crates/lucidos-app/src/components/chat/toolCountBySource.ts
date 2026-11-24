import { parseMcpToolName } from '../../utils/mcpToolName';

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
