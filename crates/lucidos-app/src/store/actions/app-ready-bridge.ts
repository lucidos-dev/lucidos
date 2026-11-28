import { appFrameFor } from '../../utils/appFrame';

export const APP_READY_MESSAGE_TYPE = 'lucidos:ui:ready';

export const APP_FRAME_READY_EVENT = 'lucidos-app-frame-ready';

export function handleAppReadyMessage(event: MessageEvent): void {
  const data = event.data as { type?: unknown } | null;
  if (!data || typeof data !== 'object' || data.type !== APP_READY_MESSAGE_TYPE) return;
  appFrameFor(event.source)?.dispatchEvent(new Event(APP_FRAME_READY_EVENT));
}
