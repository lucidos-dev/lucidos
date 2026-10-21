import {
  WIDGET_CONTENT_SIZE_MESSAGE_TYPE,
  WIDGET_MENU_MESSAGE_TYPE,
  type ContentSize,
} from '@lucidos/widget-frame';
import { appFrameFor } from '../../utils/appFrame';
import { topPanelOverlay } from '../overlayStack';
import { notePressHeldInFrame } from '../../utils/pointerPress';
import type { ViewportPoint } from '../../hooks/useAnchoredPopover';

/** Dispatched on the sending frame's own element, and bubbles to the
 *  `WidgetFrame` around it. `detail` is the `ContentSize` in CSS pixels.
 *  Keyed to the element, so a frame that has since been replaced cannot
 *  resize its successor. */
export const APP_FRAME_SIZE_EVENT = 'lucidos-app-frame-size';

/** Dispatched the same way when a frame marked `menu` asks for the widget's
 *  menu. `detail` is the `ViewportPoint` to open it at, or null for a held
 *  finger or a keyboard-raised menu. */
export const APP_FRAME_MENU_EVENT = 'lucidos-app-frame-menu';

/** Above this a report is noise: the card clips far lower anyway. */
const MAX_REPORTED_PX = 20_000;

const isLength = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v) && v >= 0;
const isCoordinate = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const clamp = (v: number, min: number, max: number) => Math.min(Math.max(v, min), max);

/** A point in the frame's own viewport, placed in the host's and clamped into
 *  the frame's box. `null` for a point that is not two finite numbers. */
function pointInViewport(frame: HTMLIFrameElement, at: unknown): ViewportPoint | null {
  const { x, y } = (at ?? {}) as { x?: unknown; y?: unknown };
  if (!isCoordinate(x) || !isCoordinate(y)) return null;
  const box = frame.getBoundingClientRect();
  return { x: clamp(box.left + x, box.left, box.right), y: clamp(box.top + y, box.top, box.bottom) };
}

/** One menu at a time: a frame behind an open overlay opens none. A frame
 *  inside the top panel, as the open shelf widget is, still may. */
function inTopPanel(frame: HTMLIFrameElement): boolean {
  const top = topPanelOverlay();
  return !top || frame.closest('[data-overlay-panel]')?.getAttribute('data-overlay-panel') === top.id;
}

/** A widget frame reporting its content size or a menu request (ADR
 *  0402). A message from any frame that is not a mounted app frame, such as a
 *  nested embed, is ignored. */
export function handleWidgetFrameMessage(event: MessageEvent): void {
  const data = event.data as { type?: unknown; height?: unknown; at?: unknown; held?: unknown } | null;
  if (!data || typeof data !== 'object') return;
  if (data.type === WIDGET_CONTENT_SIZE_MESSAGE_TYPE) {
    if (!isLength(data.height)) return;
    const size: ContentSize = { height: Math.min(data.height, MAX_REPORTED_PX) };
    appFrameFor(event.source)?.dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: size, bubbles: true }));
  } else if (data.type === WIDGET_MENU_MESSAGE_TYPE) {
    const frame = appFrameFor(event.source);
    if (!frame || !inTopPanel(frame)) return;
    const at = data.at === undefined ? null : pointInViewport(frame, data.at);
    if (data.at !== undefined && !at) return;
    if (data.held === true) notePressHeldInFrame();
    frame.dispatchEvent(new CustomEvent(APP_FRAME_MENU_EVENT, { detail: at, bubbles: true }));
  }
}
