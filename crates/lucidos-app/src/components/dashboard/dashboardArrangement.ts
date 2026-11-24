
import type { DashboardTile } from '../../api/client/widgets';
import type { WidgetSize } from '../../store/types';
import { dashboardTileKey } from '../../store/widgets';

export function moveTile<T>(order: readonly T[], item: T, toIndex: number): T[] {
  const from = order.indexOf(item);
  if (from < 0) return [...order];
  const rest = order.filter((_, i) => i !== from);
  const to = Math.max(0, Math.min(rest.length, toIndex));
  return [...rest.slice(0, to), item, ...rest.slice(to)];
}

export function inOrder(tiles: readonly DashboardTile[], keys: readonly string[]): DashboardTile[] {
  const byKey = new Map(tiles.map((t) => [dashboardTileKey(t), t]));
  return keys.flatMap((k) => byKey.get(k) ?? []);
}

export function withTile(tiles: readonly DashboardTile[], key: string, patch: { size: WidgetSize }): DashboardTile[] {
  return tiles.map((t) => {
    if (dashboardTileKey(t) !== key) return t;
    if (t.kind !== 'widget' || !t.sizes.includes(patch.size)) return t;
    return { ...t, ...patch };
  });
}
