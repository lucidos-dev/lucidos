/** What a thread's widgets read, and how SSE keeps them fresh (ADRs 0402, 0407).
 *  What the user does to a widget lives in `widget-actions.ts`, so the SSE
 *  path never loads the menus' imports into the entry chunk. */

import { fetchThreadWidgets, listReusableWidgetsApi } from '../../api/client/widgets';
import { loadingIfFresh, toFailed } from '../types';
import {
  appliedThreadWidgetTickets,
  openShelfWidget,
  reusableWidgets,
  threadWidgets,
  threadWidgetsFor,
} from '../widgets';

function setThreadWidgets(threadId: string, value: ReturnType<typeof threadWidgetsFor>): void {
  const next = new Map(threadWidgets.value);
  next.set(threadId, value);
  threadWidgets.value = next;
}

let lastTicket = 0;
/** Per thread, the newest read started. Only its answer is applied, so an
 *  older read that lands late cannot put back a chip a newer one removed. */
const newestThreadWidgetsRead = new Map<string, number>();

/** Start a read of a thread's widgets and return its ticket at once. A
 *  re-read keeps the chips and cards on screen meanwhile. */
export function rereadThreadWidgets(threadId: string): number {
  const ticket = ++lastTicket;
  newestThreadWidgetsRead.set(threadId, ticket);
  setThreadWidgets(threadId, loadingIfFresh(threadWidgetsFor(threadId)));
  void fetchThreadWidgets(threadId).then(
    (data) => {
      if (newestThreadWidgetsRead.get(threadId) !== ticket) return;
      setThreadWidgets(threadId, { status: 'loaded', data });
      const applied = new Map(appliedThreadWidgetTickets.value);
      applied.set(threadId, ticket);
      appliedThreadWidgetTickets.value = applied;
    },
    (error) => {
      if (newestThreadWidgetsRead.get(threadId) !== ticket) return;
      // A re-read that fails keeps what it had; only a first read fails.
      if (threadWidgetsFor(threadId).status !== 'loaded') setThreadWidgets(threadId, toFailed(error));
    },
  );
  return ticket;
}

/** A widget event for this thread arrived over SSE, or the stream came back
 *  after a gap. Only widgets this device has read are re-read, so an unopened
 *  thread costs nothing. */
export function onWidgetEvent(threadId: string): void {
  if (threadWidgetsFor(threadId).status !== 'not-loaded') rereadThreadWidgets(threadId);
}

/** An app changed or went away. Re-read each thread whose widgets include
 *  it, and the reusable list, which a widget may have just joined or left. */
export function refreshThreadWidgetsNaming(appId: string): void {
  for (const [threadId, widgets] of threadWidgets.value) {
    if (widgets.status === 'loaded' && widgets.data.some((w) => w.app_id === appId)) {
      rereadThreadWidgets(threadId);
    }
  }
  if (reusableWidgets.value.status !== 'not-loaded') void loadReusableWidgets();
}

/** Deleted threads take their widgets with them. */
export function forgetThreadWidgets(threadIds: readonly string[]): void {
  const remaining = new Map(threadWidgets.value);
  for (const id of threadIds) {
    remaining.delete(id);
    newestThreadWidgetsRead.delete(id);
  }
  threadWidgets.value = remaining;
  if (openShelfWidget.value && threadIds.includes(openShelfWidget.value.threadId)) closeShelfWidget();
}

export async function loadReusableWidgets(): Promise<void> {
  reusableWidgets.value = loadingIfFresh(reusableWidgets.value);
  try {
    reusableWidgets.value = { status: 'loaded', data: await listReusableWidgetsApi() };
  } catch (error) {
    if (reusableWidgets.value.status !== 'loaded') reusableWidgets.value = toFailed(error);
  }
}

export function closeShelfWidget(): void {
  openShelfWidget.value = null;
}
