/** Pure edits of a dashboard arrangement (ADR 0496). Each returns the whole
 *  tile list in order, which is what one `DashboardArranged` records. */

import type { DashboardTile } from '../../api/client/widgets';
import type { WidgetSize } from '../../store/types';
import { threadWidgetKey } from '../../store/widgets';

/** `item` moved to `toIndex`, the rest keeping their order. An index past
 *  either end is clamped, and an unknown item changes nothing. */
export function moveTile<T>(order: readonly T[], item: T, toIndex: number): T[] {
  const from = order.indexOf(item);
  if (from < 0) return [...order];
  const rest = order.filter((_, i) => i !== from);
  const to = Math.max(0, Math.min(rest.length, toIndex));
  return [...rest.slice(0, to), item, ...rest.slice(to)];
}

/** The tiles in the order their keys give. */
export function inOrder(tiles: readonly DashboardTile[], keys: readonly string[]): DashboardTile[] {
  const byKey = new Map(tiles.map((t) => [threadWidgetKey(t), t]));
  return keys.flatMap((k) => byKey.get(k) ?? []);
}

/** One tile changed by `patch`. A size the widget does not declare is refused
 *  by returning the tiles unchanged, so no surface can offer one. */
export function withTile(tiles: readonly DashboardTile[], key: string, patch: { size: WidgetSize }): DashboardTile[] {
  return tiles.map((t) => {
    if (threadWidgetKey(t) !== key) return t;
    if (!t.sizes.includes(patch.size)) return t;
    return { ...t, ...patch };
  });
}
