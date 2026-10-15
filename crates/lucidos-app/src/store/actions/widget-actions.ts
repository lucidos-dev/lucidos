/** What the user does to a thread's widget from its inline widget, its chip
 *  and its menus (ADRs 0402, 0407). Every menu action is also an agent action
 *  and a CLI verb. */

import {
  getAppApi,
  makeWidgetReusableApi,
  pinWidgetApi,
  stopReusingWidgetApi,
  unpinWidgetApi,
  type WidgetInstanceRef,
} from '../../api/client/widgets';
import { widgetInstanceKey, type WidgetParams } from '../../utils/widgetParams';
import { expandExchange, showToast, threadMap } from '../store';
import { computeExchanges } from '../thread-events';
import { openShelfWidget } from '../widgets';
import { errorDetail } from '../../utils/errorDetail';
import { openApp } from './apps';
import { appendMessagesToCompose } from './chat';
import { focusPromptNow } from '../../components/chat/promptFocus';
import { showEventWhereItLives } from './event-navigation';
import { closeShelfWidget } from './widgets';
import { homeThreadId, isHomeThread } from './homeThread';

/** A chip tap: drop the instance open under the title, or fold it back. */
export function toggleShelfWidget(threadId: string, instanceKey: string): void {
  const open = openShelfWidget.value;
  openShelfWidget.value = open?.threadId === threadId && open.instanceKey === instanceKey
    ? null
    : { threadId, instanceKey };
}

/** Run a widget action, toasting a failure. */
async function run(action: () => Promise<unknown>, failure: string): Promise<void> {
  try {
    await action();
  } catch (error) {
    showToast(`${failure}: ${errorDetail(error)}`, 'error');
  }
}

/** Where a pin in this thread puts a widget, as the user reads it. Home draws
 *  no shelf: its pins are tiles on the Home board. */
function pinPlace(threadId: string): string {
  return isHomeThread(threadId) ? 'Home' : 'the shelf';
}

/** "Unpin from shelf": the chip goes, the inline widget and the files stay. */
export async function unpinWidgetFromShelf(instance: WidgetInstanceRef, name: string): Promise<void> {
  const open = openShelfWidget.value;
  if (open?.threadId === instance.threadId && open.instanceKey === widgetInstanceKey(instance.appId, instance.params)) {
    closeShelfWidget();
  }
  await run(() => unpinWidgetApi(instance), `Couldn't unpin widget "${name}" from ${pinPlace(instance.threadId)}`);
}

/** "Pin to shelf": the instance gets a chip. `shownEventId` is the event that
 *  draws it in the thread, where "Show in thread" lands. */
export async function pinWidgetToShelf(instance: WidgetInstanceRef, name: string, shownEventId?: string): Promise<void> {
  await run(() => pinWidgetApi(instance, shownEventId), `Couldn't pin widget "${name}" to ${pinPlace(instance.threadId)}`);
}

/** "Pin to Home" from another thread: a tile on the Home board. The engine makes
 *  a thread's own widget reusable in the same action, and the toast says so.
 *  Deleting that thread then never empties the tile (ADR 0432). */
export async function pinWidgetToHome(instance: Omit<WidgetInstanceRef, 'threadId'>, name: string): Promise<void> {
  const home = homeThreadId.value;
  if (!home) return;
  await run(async () => {
    const answer = await pinWidgetApi({ ...instance, threadId: home }) as { made_reusable?: boolean } | null;
    showToast(answer?.made_reusable
      ? `Pinned "${name}" to Home and made it reusable, so it stays if its thread is deleted.`
      : `Pinned "${name}" to Home.`, 'success');
  }, `Couldn't pin widget "${name}" to Home`);
}

export async function makeWidgetReusable(appId: string, name: string): Promise<void> {
  await run(() => makeWidgetReusableApi(appId), `Couldn't make widget "${name}" reusable`);
}

export async function stopReusingWidget(appId: string, name: string): Promise<void> {
  await run(() => stopReusingWidgetApi(appId), `Couldn't stop reusing widget "${name}"`);
}

/** "Open in Canvas": the widget instance full size in the content pane, with
 *  its params. The apps list never holds a widget, so it is read by id. */
export async function openWidgetInCanvas(appId: string, name: string, params?: WidgetParams): Promise<void> {
  try {
    openApp(await getAppApi(appId), undefined, params);
  } catch (error) {
    showToast(`Couldn't open widget "${name}": ${errorDetail(error)}`, 'error');
  }
}

/** "Make app": a prompt in this thread's composer, which the user can add to
 *  before sending. Nothing is written until the agent builds the new app, so
 *  the widget itself never changes. */
export function makeAppFromWidget(threadId: string, appId: string, name: string): void {
  appendMessagesToCompose(threadId, [
    { text: `Make an app from the ${name} widget (\`${appId}\`).`, imageHashes: [] },
  ]);
  focusPromptNow();
}

/** "Show in thread": land on the inline widget, at the turn that drew it. A
 *  folded turn draws no body, so that turn unfolds first. The id is an old
 *  `WidgetShown` step, or the turn's starter, which a reply's pin records. */
export function showWidgetInThread(threadId: string, shownEventId: string): void {
  closeShelfWidget();
  const thread = threadMap.value.get(threadId);
  const turn = thread && computeExchanges(thread).find((x) => x.userEvent._eventId === shownEventId
    || x.steps.some(({ event }) => event._eventId === shownEventId));
  if (turn) expandExchange(threadId, turn.userSeq);
  void showEventWhereItLives(shownEventId);
}
