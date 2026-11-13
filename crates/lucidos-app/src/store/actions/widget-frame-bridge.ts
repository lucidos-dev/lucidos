import {
  FIT_EDGES,
  FIT_KINDS,
  MAX_FIT_FINDINGS,
  WIDGET_CONTENT_SIZE_MESSAGE_TYPE,
  WIDGET_FIT_MESSAGE_TYPE,
  WIDGET_MENU_MESSAGE_TYPE,
  type ContentSize,
  type FitFinding,
  type WidgetSize,
} from '@lucidos/widget-frame';
import { WIDGET_SIZES } from '@lucidos/engine-constants';
import { appFrameFor } from '../../utils/appFrame';
import { topPanelOverlay } from '../overlayStack';
import { notePressOwesClick } from '../../utils/pointerPress';
import type { ViewportPoint } from '../../hooks/useAnchoredPopover';

/** Dispatched on the sending frame's own element, and bubbles to the
 *  `WidgetFrame` around it. `detail` is the `ContentSize`: CSS pixels, and
 *  the widget size laid out for when the report names a valid one.
 *  Keyed to the element, so a frame that has since been replaced cannot
 *  resize its successor. */
export const APP_FRAME_SIZE_EVENT = 'lucidos-app-frame-size';

/** Dispatched the same way when a frame marked `menu` asks for the widget's
 *  menu. `detail` is the `ViewportPoint` to open it at, or null for a held
 *  finger or a keyboard-raised menu. */
export const APP_FRAME_MENU_EVENT = 'lucidos-app-frame-menu';

/** Dispatched the same way when a frame answers a push's `measure` token.
 *  `detail` is the `WidgetFit` it reported. */
export const APP_FRAME_FIT_EVENT = 'lucidos-app-frame-fit';

/** What a frame measured for one `measure` token: its box and what the box
 *  cuts off, or why it could not measure. */
export type WidgetFit = { token: string; size?: WidgetSize }
  & ({ box: { width: number; height: number }; findings: FitFinding[]; total: number } | { error: string });

/** Above this a report is noise: the card clips far lower anyway. */
const MAX_REPORTED_PX = 20_000;

const isLength = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v) && v >= 0;
const isCoordinate = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const isWidgetSize = (v: unknown): v is WidgetSize => WIDGET_SIZES.includes(v as WidgetSize);
const isText = (v: unknown): v is string => typeof v === 'string';
const clamp = (v: number, min: number, max: number) => Math.min(Math.max(v, min), max);

/** A point in the frame's own viewport, placed in the host's and clamped into
 *  the frame's box. `null` for a point that is not two finite numbers. */
function pointInViewport(frame: HTMLIFrameElement, at: unknown): ViewportPoint | null {
  const { x, y } = (at ?? {}) as { x?: unknown; y?: unknown };
  if (!isCoordinate(x) || !isCoordinate(y)) return null;
  const box = frame.getBoundingClientRect();
  return { x: clamp(box.left + x, box.left, box.right), y: clamp(box.top + y, box.top, box.bottom) };
}

function isFinding(v: unknown): v is FitFinding {
  const f = (v ?? {}) as Partial<Record<keyof FitFinding, unknown>>;
  return FIT_KINDS.includes(f.kind as FitFinding['kind']) && FIT_EDGES.includes(f.edge as FitFinding['edge'])
    && isLength(f.px) && isText(f.path) && isText(f.text);
}

/** A fit report checked field by field, or null when any field is malformed. */
function fitOf(data: Record<string, unknown>): WidgetFit | null {
  if (!isText(data.token) || !data.token) return null;
  const head = { token: data.token, ...(isWidgetSize(data.size) ? { size: data.size } : {}) };
  if ('error' in data) return isText(data.error) ? { ...head, error: data.error } : null;
  const { width, height } = (data.box ?? {}) as { width?: unknown; height?: unknown };
  if (!isLength(width) || !isLength(height) || !isLength(data.total) || !Array.isArray(data.findings)) return null;
  if (!data.findings.every(isFinding)) return null;
  return { ...head, box: { width, height }, findings: data.findings.slice(0, MAX_FIT_FINDINGS), total: data.total };
}

/** One menu at a time: a frame behind an open overlay opens none. A frame
 *  inside the top panel still may. */
function inTopPanel(frame: HTMLIFrameElement): boolean {
  const top = topPanelOverlay();
  return !top || frame.closest('[data-overlay-panel]')?.getAttribute('data-overlay-panel') === top.id;
}

/** A widget frame reporting its content size, a menu request (ADR 0402),
 *  or what its box cuts off. A message from any frame that is not a mounted app frame, such as a
 *  nested embed, is ignored. */
export function handleWidgetFrameMessage(event: MessageEvent): void {
  const data = event.data as {
    type?: unknown; width?: unknown; height?: unknown; size?: unknown; at?: unknown; held?: unknown;
  } | null;
  if (!data || typeof data !== 'object') return;
  if (data.type === WIDGET_CONTENT_SIZE_MESSAGE_TYPE) {
    if (!isLength(data.height)) return;
    const size: ContentSize = { height: Math.min(data.height, MAX_REPORTED_PX) };
    if (isLength(data.width)) size.width = Math.min(data.width, MAX_REPORTED_PX);
    if (isWidgetSize(data.size)) size.size = data.size;
    appFrameFor(event.source)?.dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: size, bubbles: true }));
  } else if (data.type === WIDGET_MENU_MESSAGE_TYPE) {
    const frame = appFrameFor(event.source);
    if (!frame || !inTopPanel(frame)) return;
    const at = data.at === undefined ? null : pointInViewport(frame, data.at);
    if (data.at !== undefined && !at) return;
    if (data.held === true) notePressOwesClick();
    frame.dispatchEvent(new CustomEvent(APP_FRAME_MENU_EVENT, { detail: at, bubbles: true }));
  } else if (data.type === WIDGET_FIT_MESSAGE_TYPE) {
    const fit = fitOf(data as Record<string, unknown>);
    if (fit) appFrameFor(event.source)?.dispatchEvent(new CustomEvent(APP_FRAME_FIT_EVENT, { detail: fit, bubbles: true }));
  }
}
