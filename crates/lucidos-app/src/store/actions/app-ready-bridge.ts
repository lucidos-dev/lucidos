import { appFrameFor } from '../../utils/appFrame';

/** What `lucidos.ui.ready()` posts (`packages/lucidos-sdk/src/ui.ts`). */
export const APP_READY_MESSAGE_TYPE = 'lucidos:ui:ready';

/** Dispatched on the sending frame's own element, where `AppUiInline` listens.
 *  Keyed to the element, so a ready from a frame that has since been replaced
 *  can never reveal the one that replaced it. */
export const APP_FRAME_READY_EVENT = 'lucidos-app-frame-ready';

/** An app saying its content is on screen. A message from any frame that is
 *  not a mounted app frame, such as a nested embed, is ignored. */
export function handleAppReadyMessage(event: MessageEvent): void {
  const data = event.data as { type?: unknown } | null;
  if (!data || typeof data !== 'object' || data.type !== APP_READY_MESSAGE_TYPE) return;
  appFrameFor(event.source)?.dispatchEvent(new Event(APP_FRAME_READY_EVENT));
}
