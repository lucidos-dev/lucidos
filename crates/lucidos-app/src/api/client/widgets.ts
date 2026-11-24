import { API, json } from './_core';
import type { App, AppReveal, TileSize, WidgetSize } from '../../store/types';
import type { Dashboard } from '../../generated/thread-event-wire';
import type { WidgetParams } from '../../utils/widgetParams';
import type { FitFinding } from '@lucidos/widget-frame';
import { readDeviceId } from '../../utils/deviceIdHeader';

export interface WidgetTile {
  app_id: string;
  params?: WidgetParams;
  label?: string;
  name: string;
  icon?: string;
  reusable: boolean;
  origin_plugin_id?: string;
  built_in?: boolean;
  reveal: AppReveal;
  sizes: WidgetSize[];
  transparent?: boolean;
}

export interface WidgetDashboardTile extends WidgetTile {
  kind: 'widget';
  size: WidgetSize;
}

export interface AppDashboardTile {
  kind: 'app';
  app_id: string;
  name: string;
  icon?: string;
  label?: string;
  size: 'quarter';
}

export type DashboardTile = WidgetDashboardTile | AppDashboardTile;

export interface DashboardView {
  dashboard: Dashboard;
  tiles: DashboardTile[];
}

export interface ArrangedTile {
  app_id: string;
  params?: WidgetParams;
  size: TileSize;
}

export function fetchDashboard(dashboard: Dashboard): Promise<DashboardView> {
  return json(`${API}/dashboard?dashboard=${dashboard}`);
}

export function arrangeDashboardApi(dashboard: Dashboard, tiles: ArrangedTile[]): Promise<DashboardView> {
  return json(`${API}/widgets/arrange`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ dashboard, tiles }),
  });
}

export function getAppApi(appId: string): Promise<App> {
  return json(`${API}/app?id=${encodeURIComponent(appId)}`);
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

export interface WidgetInstanceRef {
  appId: string;
  threadId: string;
  params?: WidgetParams;
}

export function sizeWidgetCardsApi({ appId, threadId, params }: WidgetInstanceRef, size: WidgetSize): Promise<unknown> {
  return post('/widgets/size', { app_id: appId, thread_id: threadId, params, size });
}

export interface DashboardTileRef {
  appId: string;
  params?: WidgetParams;
  label?: string;
  size?: WidgetSize;
}

export function addToDashboardApi(dashboard: Dashboard, { appId, params, label, size }: DashboardTileRef): Promise<{ made_reusable: boolean }> {
  return post('/widgets/dashboard-add', { app_id: appId, params, label, dashboard, size }) as Promise<{ made_reusable: boolean }>;
}

export function removeFromDashboardApi(dashboard: Dashboard, { appId, params }: DashboardTileRef): Promise<unknown> {
  return post('/widgets/dashboard-remove', { app_id: appId, params, dashboard });
}

export function makeWidgetReusableApi(appId: string): Promise<unknown> {
  return post('/widgets/make-reusable', { app_id: appId });
}

export function stopReusingWidgetApi(appId: string): Promise<unknown> {
  return post('/widgets/stop-reusing', { app_id: appId });
}

export type WidgetFitSurface = 'desk-tile' | 'phone-column';

export type WidgetFitBox = { surface: WidgetFitSurface; size: WidgetSize }
  & ({ box: { width: number; height: number }; findings: FitFinding[]; total: number } | { error: string });

export function postWidgetFitReportApi(requestId: string, boxes: WidgetFitBox[]): Promise<unknown> {
  return post('/widgets/fit-report', { request_id: requestId, device_id: readDeviceId(), boxes });
}
