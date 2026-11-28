
import { signal } from '@preact/signals';
import type { DashboardTile } from '../api/client/widgets';
import type { App, Loadable } from './types';
import { widgetInstanceKey } from '../utils/widgetParams';

export const reusableWidgets = signal<Loadable<App[]>>({ status: 'not-loaded' });

export const widgetRefreshKeys = signal<ReadonlyMap<string, number>>(new Map());

export function dashboardTileKey(tile: DashboardTile): string {
  return widgetInstanceKey(tile.app_id, tile.kind === 'widget' ? tile.params : undefined);
}

export function widgetRefreshKey(appId: string): number {
  return widgetRefreshKeys.value.get(appId) ?? 0;
}

export function reloadWidgetFrames(appId: string): void {
  const keys = new Map(widgetRefreshKeys.value);
  keys.set(appId, (keys.get(appId) ?? 0) + 1);
  widgetRefreshKeys.value = keys;
}
