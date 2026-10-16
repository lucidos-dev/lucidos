import {
  WIDGET_CONTENT_SIZE_MESSAGE_TYPE,
  WIDGET_HOLD_MESSAGE_TYPE,
  WIDGET_TAP_MESSAGE_TYPE,
  type ContentSize,
} from '@lucidos/widget-frame';
import { appFrameFor } from '../../utils/appFrame';

/** Dispatched on the sending frame's own element, and bubbles to the
 *  `WidgetFrame` around it. `detail` is the `ContentSize` in CSS pixels.
 *  Keyed to the element, so a frame that has since been replaced cannot
 *  resize its successor. */
export const APP_FRAME_SIZE_EVENT = 'lucidos-app-frame-size';

/** Dispatched the same way when a frame on a dashboard tile is tapped.
 *  `detail` is the tap's pointer type. */
export const APP_FRAME_TAP_EVENT = 'lucidos-app-frame-tap';

/** Dispatched the same way when a finger holds still on a dashboard tile. */
export const APP_FRAME_HOLD_EVENT = 'lucidos-app-frame-hold';

/** Above this a report is noise: the card clips far lower anyway. */
const MAX_REPORTED_PX = 20_000;

const isLength = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v) && v >= 0;

/** A widget frame reporting its content size or a press (ADR 0402). A message
 *  from any frame that is not a mounted app frame, such as a nested embed, is
 *  ignored. */
export function handleWidgetFrameMessage(event: MessageEvent): void {
  const data = event.data as { type?: unknown; height?: unknown; pointerType?: unknown } | null;
  if (!data || typeof data !== 'object') return;
  if (data.type === WIDGET_CONTENT_SIZE_MESSAGE_TYPE) {
    if (!isLength(data.height)) return;
    const size: ContentSize = { height: Math.min(data.height, MAX_REPORTED_PX) };
    appFrameFor(event.source)?.dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: size, bubbles: true }));
  } else if (data.type === WIDGET_TAP_MESSAGE_TYPE) {
    const pointerType = typeof data.pointerType === 'string' ? data.pointerType : '';
    appFrameFor(event.source)?.dispatchEvent(new CustomEvent(APP_FRAME_TAP_EVENT, { detail: pointerType, bubbles: true }));
  } else if (data.type === WIDGET_HOLD_MESSAGE_TYPE) {
    appFrameFor(event.source)?.dispatchEvent(new CustomEvent(APP_FRAME_HOLD_EVENT, { bubbles: true }));
  }
}
