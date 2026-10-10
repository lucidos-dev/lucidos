/** Widget state (ADRs 0402, 0407): each thread's widgets, the widget a
 *  shelf chip dropped open, and the reusable widgets the apps panel lists.
 *
 *  The engine derives a thread's widgets from its widget events plus each
 *  widget's app manifest. So a reload or another device reads the same ones.
 *  The client keeps the last answer per thread and re-reads it when a widget
 *  event for that thread arrives over SSE. */

import { signal } from '@preact/signals';
import type { ThreadWidget } from '../api/client/widgets';
import type { App, Loadable } from './types';

export const threadWidgets = signal<ReadonlyMap<string, Loadable<ThreadWidget[]>>>(new Map());

/** Per thread, the ticket of the widgets read whose answer is on screen. A read
 *  started later has a higher ticket. So a reader can tell "absent from a
 *  fresh read" from "absent from a stale one". */
export const appliedThreadWidgetTickets = signal<ReadonlyMap<string, number>>(new Map());

/** The widget a chip tap dropped open under a thread's title, if any. */
export const openShelfWidget = signal<{ threadId: string; appId: string } | null>(null);

export const reusableWidgets = signal<Loadable<App[]>>({ status: 'not-loaded' });

/** Per widget, bumped when its files change, so every frame showing it
 *  reloads. The Canvas pane's own key is `appRefreshKey`. */
export const widgetRefreshKeys = signal<ReadonlyMap<string, number>>(new Map());

export function threadWidgetsFor(threadId: string): Loadable<ThreadWidget[]> {
  return threadWidgets.value.get(threadId) ?? { status: 'not-loaded' };
}

/** One widget in one thread, once the thread's widgets are loaded. */
export function threadWidget(threadId: string, appId: string): ThreadWidget | undefined {
  const widgets = threadWidgetsFor(threadId);
  return widgets.status === 'loaded' ? widgets.data.find((w) => w.app_id === appId) : undefined;
}

export function appliedThreadWidgetTicket(threadId: string): number {
  return appliedThreadWidgetTickets.value.get(threadId) ?? 0;
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
