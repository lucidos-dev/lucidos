/** What the user does to a thread's widget from its inline widget and its
 *  menus (ADR 0402). Every menu action is also an agent action and a CLI
 *  verb. */

import {
  getAppApi,
  makeWidgetReusableApi,
  sizeWidgetCardsApi,
  stopReusingWidgetApi,
  type WidgetInstanceRef,
} from '../../api/client/widgets';
import type { WidgetParams } from '../../utils/widgetParams';
import type { WidgetSize } from '../types';
import { showToast } from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { openApp } from './apps';
import { appendMessagesToCompose } from './chat';
import { focusPromptNow } from '../../components/chat/promptFocus';

/** Run a widget action, toasting a failure. */
async function run(action: () => Promise<unknown>, failure: string): Promise<void> {
  try {
    await action();
  } catch (error) {
    showToast(`${failure}: ${errorDetail(error)}`, 'error');
  }
}

/** A pill pick: this instance's cards in the thread take `size`. The card
 *  redraws when the event arrives, so a refusal changes nothing on screen. */
export async function sizeWidgetCards(instance: WidgetInstanceRef, name: string, size: WidgetSize): Promise<void> {
  await run(() => sizeWidgetCardsApi(instance, size), `Couldn't resize widget "${name}"`);
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
