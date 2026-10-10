import { API, json } from './_core';
import type { App, AppReveal } from '../../store/types';

/** One widget shown in a thread, chip or not (`engine::widgets::ThreadWidget`). */
export interface ThreadWidget {
  app_id: string;
  name: string;
  /** The widget's app icon, as `App.icon`. */
  icon?: string;
  reusable: boolean;
  /** When the frame's cover lifts, from the manifest, as for any app. */
  reveal: AppReveal;
  /** Pinned to the shelf, so it has a chip there. The thread's card shows
   *  either way. */
  pinned: boolean;
  /** The newest `WidgetShown` for this widget in this thread. */
  shown_event_id: string;
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

function post(path: string, body: Record<string, string>): Promise<unknown> {
  return json(`${API}${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

/** "Pin to shelf": one thread event, no file change. */
export function pinWidgetApi(appId: string, threadId: string): Promise<unknown> {
  return post('/widgets/pin', { app_id: appId, thread_id: threadId });
}

/** "Unpin from shelf": one thread event, no file change. */
export function unpinWidgetApi(appId: string, threadId: string): Promise<unknown> {
  return post('/widgets/unpin', { app_id: appId, thread_id: threadId });
}

export function makeWidgetReusableApi(appId: string): Promise<unknown> {
  return post('/widgets/make-reusable', { app_id: appId });
}

export function stopReusingWidgetApi(appId: string): Promise<unknown> {
  return post('/widgets/stop-reusing', { app_id: appId });
}
