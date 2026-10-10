import { WIDGET_HEIGHT_MESSAGE_TYPE } from '@lucidos/widget-height';
import { appFrameFor } from '../../utils/appFrame';

/** Dispatched on the sending frame's own element, and bubbles to the
 *  `WidgetFrame` around it. `detail` is the content height in CSS pixels.
 *  Keyed to the element, so a frame that has since been replaced cannot
 *  resize its successor. */
export const APP_FRAME_HEIGHT_EVENT = 'lucidos-app-frame-height';

/** Above this a report is noise: the card clips far lower anyway. */
const MAX_REPORTED_HEIGHT_PX = 20_000;

/** A widget frame reporting its content height (ADR 0402). A message from any
 *  frame that is not a mounted app frame, such as a nested embed, is ignored. */
export function handleAppHeightMessage(event: MessageEvent): void {
  const data = event.data as { type?: unknown; height?: unknown } | null;
  if (!data || typeof data !== 'object' || data.type !== WIDGET_HEIGHT_MESSAGE_TYPE) return;
  const height = data.height;
  if (typeof height !== 'number' || !Number.isFinite(height) || height < 0) return;
  appFrameFor(event.source)?.dispatchEvent(
    new CustomEvent(APP_FRAME_HEIGHT_EVENT, { detail: Math.min(height, MAX_REPORTED_HEIGHT_PX), bubbles: true }),
  );
}
