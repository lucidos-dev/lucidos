import { API, json } from './_core';
import type { App, AppReveal } from '../../store/types';
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
  /** Pinned to the shelf, so it has a chip there. The thread's card shows
   *  either way. */
  pinned: boolean;
  /** The newest `WidgetShown` for this instance in this thread. Absent for
   *  an instance only ever pinned, as from an embed. */
  shown_event_id?: string;
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

/** One widget instance in one thread, as show, pin and unpin name it. */
export interface WidgetInstanceRef {
  appId: string;
  threadId: string;
  params?: WidgetParams;
  label?: string;
}

function instanceBody({ appId, threadId, params, label }: WidgetInstanceRef): Record<string, unknown> {
  return { app_id: appId, thread_id: threadId, params, label };
}

/** "Pin to shelf": one thread event, no file change. */
export function pinWidgetApi(instance: WidgetInstanceRef): Promise<unknown> {
  return post('/widgets/pin', instanceBody(instance));
}

/** "Unpin from shelf": one thread event, no file change. */
export function unpinWidgetApi(instance: WidgetInstanceRef): Promise<unknown> {
  return post('/widgets/unpin', instanceBody(instance));
}

export function makeWidgetReusableApi(appId: string): Promise<unknown> {
  return post('/widgets/make-reusable', { app_id: appId });
}

export function stopReusingWidgetApi(appId: string): Promise<unknown> {
  return post('/widgets/stop-reusing', { app_id: appId });
}
