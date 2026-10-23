import { API, json } from './_core';
import type { App, AppReveal, TileSize, WidgetSize } from '../../store/types';
import type { Dashboard } from '../../generated/thread-event-wire';
import type { WidgetParams } from '../../utils/widgetParams';

/** One widget instance in a thread, chip or not (`engine::widgets::ThreadWidget`). */
export interface ThreadWidget {
  app_id: string;
  /** The instance's params (ADR 0415). Absent means none. */
  params?: WidgetParams;
  /** The instance's chip label. Absent means the widget's name. */
  label?: string;
  name: string;
  /** The widget's app icon, as `App.icon`. */
  icon?: string;
  reusable: boolean;
  /** The plugin that ships this widget. Such a widget offers no Stop reusing. */
  origin_plugin_id?: string;
  /** A *built-in widget*, which no menu may change (ADR 0415). */
  built_in?: boolean;
  /** When the frame's cover lifts, from the manifest, as for any app. */
  reveal: AppReveal;
  /** Its *widget sizes*, first the default (ADR 0432). */
  sizes: WidgetSize[];
  /** Its dashboard tile is see-through, so it draws its own shape. */
  transparent?: boolean;
  /** Pinned to the shelf, so it has a chip there. The inline widget shows
   *  either way. */
  pinned: boolean;
  /** Where "Show in thread" lands: the reply the pin named, or an old
   *  `WidgetShown`. Absent for an instance pinned from nowhere in the thread. */
  shown_event_id?: string;
}

/** A widget tile (`engine::dashboard::DashboardTile::Widget`): the widget as a
 *  shelf chip reads it, at the size it shows at there. It always names a
 *  reusable widget, so no thread holds it. */
export interface WidgetDashboardTile extends ThreadWidget {
  kind: 'widget';
  size: WidgetSize;
}

/** An *app shortcut* (`engine::dashboard::AppShortcut`): an app's icon and
 *  name, always a quarter tile. It takes no params. */
export interface AppDashboardTile {
  kind: 'app';
  app_id: string;
  name: string;
  /** The app's icon, as `App.icon`. */
  icon?: string;
  /** The shortcut's label. Absent means the app's name. */
  label?: string;
  size: 'quarter';
}

/** One tile on a dashboard (`engine::dashboard::DashboardTile`). */
export type DashboardTile = WidgetDashboardTile | AppDashboardTile;

/** One dashboard's tiles, in order (`engine::dashboard::DashboardView`). */
export interface DashboardView {
  dashboard: Dashboard;
  tiles: DashboardTile[];
}

/** One tile of an arrangement: a widget instance or an app, and the size it
 *  shows at. */
export interface ArrangedTile {
  app_id: string;
  params?: WidgetParams;
  size: TileSize;
}

export function fetchDashboard(dashboard: Dashboard): Promise<DashboardView> {
  return json(`${API}/dashboard?dashboard=${dashboard}`);
}

/** Write one `DashboardArranged` and answer the dashboard it gives. */
export function arrangeDashboardApi(dashboard: Dashboard, tiles: ArrangedTile[]): Promise<DashboardView> {
  return json(`${API}/widgets/arrange`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ dashboard, tiles }),
  });
}

/** One app by id, a widget included. The apps list never holds a widget. */
export function getAppApi(appId: string): Promise<App> {
  return json(`${API}/app?id=${encodeURIComponent(appId)}`);
}

export function fetchThreadWidgets(threadId: string): Promise<ThreadWidget[]> {
  return json(`${API}/widgets/thread?thread_id=${encodeURIComponent(threadId)}`);
}

export function listReusableWidgetsApi(): Promise<App[]> {
  return json(`${API}/widgets`);
}

function post(path: string, body: Record<string, unknown>): Promise<unknown> {
  return json(`${API}${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

/** One widget instance in one thread, as pin and unpin name it. */
export interface WidgetInstanceRef {
  appId: string;
  threadId: string;
  params?: WidgetParams;
  label?: string;
}

function instanceBody({ appId, threadId, params, label }: WidgetInstanceRef): Record<string, unknown> {
  return { app_id: appId, thread_id: threadId, params, label };
}

/** "Pin to shelf": one thread event, no file change. `shownEventId` is the
 *  reply that draws the widget, where "Show in thread" lands. */
export function pinWidgetApi(instance: WidgetInstanceRef, shownEventId?: string): Promise<unknown> {
  return post('/widgets/pin', { ...instanceBody(instance), shown_event_id: shownEventId });
}

/** "Unpin from shelf": one thread event, no file change. */
export function unpinWidgetApi(instance: WidgetInstanceRef): Promise<unknown> {
  return post('/widgets/unpin', instanceBody(instance));
}

/** One widget instance on a dashboard, as add and remove name it. */
export interface DashboardTileRef {
  appId: string;
  params?: WidgetParams;
  label?: string;
}

/** "Add to dashboard". A widget that belongs to a thread is made reusable
 *  first, and `made_reusable` says so. */
export function addToDashboardApi(dashboard: Dashboard, { appId, params, label }: DashboardTileRef): Promise<{ made_reusable: boolean }> {
  return post('/widgets/dashboard-add', { app_id: appId, params, label, dashboard }) as Promise<{ made_reusable: boolean }>;
}

/** "Remove from dashboard": the tile goes, the widget stays. */
export function removeFromDashboardApi(dashboard: Dashboard, { appId, params }: DashboardTileRef): Promise<unknown> {
  return post('/widgets/dashboard-remove', { app_id: appId, params, dashboard });
}

export function makeWidgetReusableApi(appId: string): Promise<unknown> {
  return post('/widgets/make-reusable', { app_id: appId });
}

export function stopReusingWidgetApi(appId: string): Promise<unknown> {
  return post('/widgets/stop-reusing', { app_id: appId });
}
