import { WIDGET_SIZE_MESSAGE_TYPE, WIDGET_TAP_MESSAGE_TYPE, type ContentSize } from '@lucidos/widget-frame';
import { appFrameFor } from '../../utils/appFrame';

/** Dispatched on the sending frame's own element, and bubbles to the
 *  `WidgetFrame` around it. `detail` is the `ContentSize` in CSS pixels.
 *  Keyed to the element, so a frame that has since been replaced cannot
 *  resize its successor. */
export const APP_FRAME_SIZE_EVENT = 'lucidos-app-frame-size';

/** Dispatched the same way when a frame in a widget window is pressed.
 *  `detail` is the press's pointer type. */
export const APP_FRAME_TAP_EVENT = 'lucidos-app-frame-tap';

/** Above this a report is noise: the card clips far lower anyway. */
const MAX_REPORTED_PX = 20_000;

const isLength = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v) && v >= 0;

/** A widget frame reporting its content size or a press (ADRs 0402, 0419). A
 *  message from any frame that is not a mounted app frame, such as a nested
 *  embed, is ignored. */
export function handleWidgetFrameMessage(event: MessageEvent): void {
  const data = event.data as { type?: unknown; height?: unknown; width?: unknown; pointerType?: unknown } | null;
  if (!data || typeof data !== 'object') return;
  if (data.type === WIDGET_SIZE_MESSAGE_TYPE) {
    if (!isLength(data.height) || (data.width !== undefined && !isLength(data.width))) return;
    const size: ContentSize = { height: Math.min(data.height, MAX_REPORTED_PX) };
    if (data.width !== undefined) size.width = Math.min(data.width, MAX_REPORTED_PX);
    appFrameFor(event.source)?.dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: size, bubbles: true }));
  } else if (data.type === WIDGET_TAP_MESSAGE_TYPE) {
    const pointerType = typeof data.pointerType === 'string' ? data.pointerType : '';
    appFrameFor(event.source)?.dispatchEvent(new CustomEvent(APP_FRAME_TAP_EVENT, { detail: pointerType, bubbles: true }));
  }
}
