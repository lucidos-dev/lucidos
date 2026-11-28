
import type { ListedWorkspace } from '../api/client/control';
import { workspaceRouteSegment } from './basePath';
import { remoteWorkspaceRoute, type RemoteGatewayView } from './remoteWorkspaces';

export type OrderedWorkspace<L> =
  | { kind: 'local'; key: string; workspace: L }
  | { kind: 'remote'; key: string; workspace: ListedWorkspace; view: RemoteGatewayView };

export function orderWorkspaces<L extends { id: string }>(
  local: readonly L[],
  remote: readonly RemoteGatewayView[],
  order: readonly string[],
): OrderedWorkspace<L>[] {
  const byDefault: OrderedWorkspace<L>[] = [
    ...local.map((workspace) => ({
      kind: 'local' as const,
      key: workspaceRouteSegment({ slug: workspace.id, gateway: null }),
      workspace,
    })),
    ...remote.flatMap((view) =>
      view.workspaces.map((workspace) => ({
        kind: 'remote' as const,
        key: remoteWorkspaceRoute(view, workspace),
        workspace,
        view,
      })),
    ),
  ];
  const byKey = new Map<string, OrderedWorkspace<L>>();
  for (const entry of byDefault) if (!byKey.has(entry.key)) byKey.set(entry.key, entry);
  const drawn: OrderedWorkspace<L>[] = [];
  const take = (key: string) => {
    const entry = byKey.get(key);
    if (entry === undefined) return;
    drawn.push(entry);
    byKey.delete(key);
  };
  order.forEach(take);
  byDefault.forEach((entry) => take(entry.key));
  return drawn;
}

export function moveOrderKey(drawn: readonly string[], key: string, index: number): string[] {
  if (!drawn.includes(key)) return [...drawn];
  const rest = drawn.filter((k) => k !== key);
  const at = Math.max(0, Math.min(index, rest.length));
  return [...rest.slice(0, at), key, ...rest.slice(at)];
}

export function stepOrderKey(drawn: readonly string[], key: string, direction: 'up' | 'down'): string[] {
  const from = drawn.indexOf(key);
  if (from < 0) return [...drawn];
  return moveOrderKey(drawn, key, Math.max(0, from + (direction === 'up' ? -1 : 1)));
}

export function keepUndrawnKeys(stored: readonly string[], reordered: readonly string[]): string[] {
  const drawn = new Set(reordered);
  const queue = [...reordered];
  const merged = [...new Set(stored)].map((key) => (drawn.has(key) ? queue.shift()! : key));
  return [...merged, ...queue];
}
