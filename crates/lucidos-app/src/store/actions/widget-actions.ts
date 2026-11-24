
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

async function run(action: () => Promise<unknown>, failure: string): Promise<void> {
  try {
    await action();
  } catch (error) {
    showToast(`${failure}: ${errorDetail(error)}`, 'error');
  }
}

export async function sizeWidgetCards(instance: WidgetInstanceRef, name: string, size: WidgetSize): Promise<void> {
  await run(() => sizeWidgetCardsApi(instance, size), `Couldn't resize widget "${name}"`);
}

export async function makeWidgetReusable(appId: string, name: string): Promise<void> {
  await run(() => makeWidgetReusableApi(appId), `Couldn't make widget "${name}" reusable`);
}

export async function stopReusingWidget(appId: string, name: string): Promise<void> {
  await run(() => stopReusingWidgetApi(appId), `Couldn't stop reusing widget "${name}"`);
}

export async function openWidgetInCanvas(appId: string, name: string, params?: WidgetParams): Promise<void> {
  try {
    openApp(await getAppApi(appId), undefined, params);
  } catch (error) {
    showToast(`Couldn't open widget "${name}": ${errorDetail(error)}`, 'error');
  }
}

export function makeAppFromWidget(threadId: string, appId: string, name: string): void {
  appendMessagesToCompose(threadId, [
    { text: `Make an app from the ${name} widget (\`${appId}\`).`, imageHashes: [] },
  ]);
  focusPromptNow();
}
