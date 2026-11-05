/** Widget state (ADR 0402): the reusable widgets the apps panel lists, and
 *  the reload key every frame of a widget shares. */

import { signal } from '@preact/signals';
import type { DashboardTile } from '../api/client/widgets';
import type { App, Loadable } from './types';
import { widgetInstanceKey } from '../utils/widgetParams';

export const reusableWidgets = signal<Loadable<App[]>>({ status: 'not-loaded' });

/** Per widget, bumped when its files change, so every frame showing it
 *  reloads. The Canvas pane's own key is `appRefreshKey`. */
export const widgetRefreshKeys = signal<ReadonlyMap<string, number>>(new Map());

/** A dashboard tile's key: a widget instance's, or an app's (it has no params). */
export function dashboardTileKey(tile: DashboardTile): string {
  return widgetInstanceKey(tile.app_id, tile.kind === 'widget' ? tile.params : undefined);
}

export function widgetRefreshKey(appId: string): number {
  return widgetRefreshKeys.value.get(appId) ?? 0;
}

/** A widget's files changed: reload every frame that shows it. */
export function reloadWidgetFrames(appId: string): void {
  const keys = new Map(widgetRefreshKeys.value);
  keys.set(appId, (keys.get(appId) ?? 0) + 1);
  widgetRefreshKeys.value = keys;
}
