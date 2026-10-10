/** What the user does to a thread's widget from its card, its chip and its
 *  menus (ADRs 0402, 0407). Every menu action is also an agent action and a
 *  CLI verb. */

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

/** "Unpin from shelf": the chip goes, the thread's card and the files stay. */
export async function unpinWidgetFromShelf(instance: WidgetInstanceRef, name: string): Promise<void> {
  const open = openShelfWidget.value;
  if (open?.threadId === instance.threadId && open.instanceKey === widgetInstanceKey(instance.appId, instance.params)) {
    closeShelfWidget();
  }
  await run(() => unpinWidgetApi(instance), `Couldn't unpin widget "${name}" from the shelf`);
}

/** "Pin to shelf": the instance gets a chip. */
export async function pinWidgetToShelf(instance: WidgetInstanceRef, name: string): Promise<void> {
  await run(() => pinWidgetApi(instance), `Couldn't pin widget "${name}" to the shelf`);
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

/** "Show in thread": land on the widget's card, at the turn that showed it. A
 *  folded turn draws no body, so that turn unfolds first. */
export function showWidgetInThread(threadId: string, shownEventId: string): void {
  closeShelfWidget();
  const thread = threadMap.value.get(threadId);
  const turn = thread && computeExchanges(thread)
    .find((x) => x.steps.some(({ event }) => event._eventId === shownEventId));
  if (turn) expandExchange(threadId, turn.userSeq);
  void showEventWhereItLives(shownEventId);
}
