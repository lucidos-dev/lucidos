/**
 * The workspace order: one list of local and remote workspaces in the user's
 * order. The picker and the switcher both draw through `orderWorkspaces`, so
 * they never disagree.
 *
 * The stored order holds order keys, the route segment each workspace
 * opens at (`workspaceRouteSegment`). A workspace with no key keeps the
 * default rule after every ordered one: local in registry order, then remote
 * by gateway. Stale and repeated keys are skipped, never drawn.
 */

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

/** `drawn` with `key` moved to `index` (clamped). Store the result as the new order. */
export function moveOrderKey(drawn: readonly string[], key: string, index: number): string[] {
  if (!drawn.includes(key)) return [...drawn];
  const rest = drawn.filter((k) => k !== key);
  const at = Math.max(0, Math.min(index, rest.length));
  return [...rest.slice(0, at), key, ...rest.slice(at)];
}

/** `drawn` with `key` one place up or down. An end stays put. */
export function stepOrderKey(drawn: readonly string[], key: string, direction: 'up' | 'down'): string[] {
  const from = drawn.indexOf(key);
  if (from < 0) return [...drawn];
  return moveOrderKey(drawn, key, Math.max(0, from + (direction === 'up' ? -1 : 1)));
}

/** `reordered`, a new order of the drawn keys, laid over `stored`. A stored key
 *  not drawn now keeps its place, such as a remote workspace whose gateway has
 *  not answered yet. Drawn keys fill the drawn ones' slots, and the rest follow. */
export function keepUndrawnKeys(stored: readonly string[], reordered: readonly string[]): string[] {
  const drawn = new Set(reordered);
  const queue = [...reordered];
  const merged = [...new Set(stored)].map((key) => (drawn.has(key) ? queue.shift()! : key));
  return [...merged, ...queue];
}
