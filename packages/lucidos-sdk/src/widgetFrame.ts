
import { onHostPush, postToHost } from './_bridge';
import { watchWidgetResources, WIDGET_RESOURCES_MESSAGE_TYPE } from './widgetResources';
export { WIDGET_RESOURCES_MESSAGE_TYPE, type WidgetResources } from './widgetResources';
import { hapticTick } from './haptic';
import { opensContextMenu, selectionCovers, shouldSuppressContextMenu, textAt, type Point } from './contextMenu';
import { runIsolated } from './_isolate';
import { WIDGET_SIZES } from './generated/engine-constants';
import { domGeometry, measureFit, settle, type FitResult } from './widgetFit';

export { FIT_EDGES, FIT_KINDS, MAX_FIT_FINDINGS, SETTLE_CAP_MS, type FitFinding } from './widgetFit';

export const WIDGET_CHANNEL = 'widget';

export const WIDGET_CONTENT_SIZE_MESSAGE_TYPE = 'lucidos:ui:size';

export const WIDGET_MENU_MESSAGE_TYPE = 'lucidos:ui:widget-menu';

export const WIDGET_FIT_MESSAGE_TYPE = 'lucidos:ui:widget-fit';

export interface WidgetMenuRequest {
  type: typeof WIDGET_MENU_MESSAGE_TYPE;
  at?: Point;
  held?: true;
}

export const LONG_PRESS_DELAY_MS = 450;

export const PRESS_MOVE_TOLERANCE_PX = 10;

export const WIDGET_SIZE_ATTRIBUTE = 'data-widget-size';

export const WIDGET_PLACE_ATTRIBUTE = 'data-widget-place';

export const WIDGET_PLACES = ['inline', 'option', 'dashboard'] as const;

export type WidgetPlace = (typeof WIDGET_PLACES)[number];

export type WidgetSize = (typeof WIDGET_SIZES)[number];

export interface WidgetPush {
  place?: WidgetPlace;
  size?: WidgetSize;
  visible?: boolean;
  menu?: boolean;
  pans?: boolean;
  measure?: string;
}

export interface ContentSize {
  width?: number;
  height: number;
  size?: WidgetSize;
}

export type WidgetFitReport = { type: typeof WIDGET_FIT_MESSAGE_TYPE; token: string; size?: WidgetSize }
  & (({ box: { width: number; height: number } } & FitResult) | { error: string });

function margins(doc: Document, start: 'marginTop' | 'marginLeft', end: 'marginBottom' | 'marginRight'): number {
  const style = doc.defaultView?.getComputedStyle(doc.body);
  return style ? (parseFloat(style[start]) || 0) + (parseFloat(style[end]) || 0) : 0;
}

export function contentHeight(doc: Document): number {
  if (!doc.body) return 0;
  return Math.ceil(doc.body.scrollHeight + margins(doc, 'marginTop', 'marginBottom'));
}

export function contentWidth(doc: Document): number {
  if (!doc.body) return 0;
  return Math.ceil(doc.body.scrollWidth + margins(doc, 'marginLeft', 'marginRight'));
}

function sizeOf(data: unknown): WidgetSize | null {
  const size = (data as { size?: unknown } | null)?.size;
  return WIDGET_SIZES.includes(size as WidgetSize) ? (size as WidgetSize) : null;
}

function placeOf(data: unknown): WidgetPlace | null {
  const place = (data as { place?: unknown } | null)?.place;
  return WIDGET_PLACES.includes(place as WidgetPlace) ? (place as WidgetPlace) : null;
}

function inBox(at: WidgetPlace | null, sized: WidgetSize | null): boolean {
  return sized !== null && (at === 'inline' || at === 'dashboard');
}

function holdStillInBox(boxed: boolean): void {
  for (const el of [document.documentElement, document.body]) {
    if (!el) continue;
    if (boxed) el.style.setProperty('overflow', 'hidden', 'important');
    else el.style.removeProperty('overflow');
  }
}

function stamp(attribute: string, value: string | null): void {
  if (value) document.documentElement.setAttribute(attribute, value);
  else document.documentElement.removeAttribute(attribute);
}

let installed = false;
let stopResources: (() => void) | null = null;
let size: WidgetSize | null = null;
let place: WidgetPlace | null = null;
let visible = true;
let menu = false;
let pans = false;
let measuring: string | null = null;
let menuPress = false;
let selectionAtPress = false;
let menuFromPointer = false;
let menuOverSelection = false;
let primaryDown = false;
let endHold: (() => void) | null = null;
let heldOpenedMenu = false;
let stylesBeforeHold: Array<[property: string, value: string, priority: string]> | null = null;
export function inSidewaysBox(): boolean {
  return pans && inBox(place, size);
}

const sizeListeners = new Set<(size: WidgetSize | null) => void>();
const visibilityListeners = new Set<(visible: boolean) => void>();

function notify<T>(listeners: Set<(value: T) => void>, value: T): void {
  for (const cb of [...listeners]) runIsolated(() => cb(value));
}

function holdNativeLongPress(): void {
  const { style } = document.documentElement;
  stylesBeforeHold ??= ['user-select', '-webkit-user-select', '-webkit-touch-callout'].map((property) =>
    [property, style.getPropertyValue(property), style.getPropertyPriority(property)]);
  for (const [property] of stylesBeforeHold) style.setProperty(property, 'none', 'important');
}

function releaseNativeLongPress(): void {
  if (!stylesBeforeHold) return;
  const { style } = document.documentElement;
  for (const [property, value, priority] of stylesBeforeHold) {
    if (value) style.setProperty(property, value, priority);
    else style.removeProperty(property);
  }
  stylesBeforeHold = null;
}

function postMenuRequest(at?: Point): void {
  const request: WidgetMenuRequest = { type: WIDGET_MENU_MESSAGE_TYPE };
  if (at) request.at = at;
  else if (primaryDown) request.held = true;
  postToHost(request);
}

function trackPress(down: PointerEvent): void {
  endHold?.();
  releaseNativeLongPress();
  if ((place !== 'inline' && place !== 'dashboard') || !down.isPrimary || down.pointerType === 'mouse' || !menu) return;
  const end = () => {
    clearTimeout(timer);
    if (endHold === end) endHold = null;
    window.removeEventListener('pointermove', move, true);
    window.removeEventListener('pointerup', lift, true);
    window.removeEventListener('pointercancel', cancel, true);
  };
  const openMenu = () => {
    end();
    heldOpenedMenu = true;
    window.getSelection()?.removeAllRanges();
    hapticTick();
    postMenuRequest();
  };
  holdNativeLongPress();
  const timer = setTimeout(openMenu, LONG_PRESS_DELAY_MS);
  endHold = end;
  const move = (e: PointerEvent) => {
    if (e.pointerId !== down.pointerId) return;
    if (Math.hypot(e.screenX - down.screenX, e.screenY - down.screenY) > PRESS_MOVE_TOLERANCE_PX) end();
  };
  const lift = (e: PointerEvent) => {
    if (e.pointerId !== down.pointerId) return;
    if (e.timeStamp - down.timeStamp < LONG_PRESS_DELAY_MS) end();
    else openMenu();
  };
  const cancel = (e: PointerEvent) => {
    if (e.pointerId === down.pointerId) end();
  };
  window.addEventListener('pointermove', move, true);
  window.addEventListener('pointerup', lift, true);
  window.addEventListener('pointercancel', cancel, true);
}

function notePress(e: PointerEvent): void {
  heldOpenedMenu = false;
  primaryDown = e.isPrimary !== false && e.button === 0;
  menuPress = opensContextMenu(e);
  selectionAtPress = menuPress && selectionCovers(window.getSelection(), { x: e.clientX, y: e.clientY });
}

function noteLift(e: PointerEvent): void {
  if (e.isPrimary !== false) primaryDown = false;
}

function takeMenuPress(): void {
  menuFromPointer = menuPress;
  menuOverSelection = selectionAtPress;
  menuPress = selectionAtPress = false;
  endHold?.();
}

function forwardContextMenu(e: MouseEvent): void {
  const fromPointer = menuFromPointer || (!primaryDown && e.buttons !== 0);
  const heldSelection = menuOverSelection;
  if (heldOpenedMenu) {
    heldOpenedMenu = false;
    e.preventDefault();
    return;
  }
  if (!menu) return;
  const point = { x: e.clientX, y: e.clientY };
  const textUnderPointer = () => !fromPointer || textAt(e.target, point);
  if (!shouldSuppressContextMenu(e, heldSelection, textUnderPointer)) return;
  e.preventDefault();
  const selection = window.getSelection();
  if (selectionCovers(selection, point)) selection?.removeAllRanges();
  postMenuRequest(fromPointer ? point : undefined);
}

async function measureWhenSettled(token: string, laidOutFor: WidgetSize | null): Promise<void> {
  await settle(document);
  if (measuring !== token) return;
  const head = { type: WIDGET_FIT_MESSAGE_TYPE, token, ...(laidOutFor ? { size: laidOutFor } : {}) } as const;
  let report: WidgetFitReport;
  try {
    report = { ...head, box: domGeometry.box(), ...measureFit(document) };
  } catch (error) {
    report = { ...head, error: error instanceof Error ? error.message : String(error) };
  }
  postToHost(report);
}

export function installWidgetFrame(): void {
  let last = '';
  let scheduled = false;
  const report = () => {
    scheduled = false;
    const content: ContentSize = { width: contentWidth(document), height: contentHeight(document) };
    if (size) content.size = size;
    const key = `${content.width} ${content.height} ${size}`;
    if (key === last) return;
    last = key;
    postToHost({ type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, ...content });
  };
  const schedule = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(report);
  };
  onHostPush(WIDGET_CHANNEL, (data) => {
    if (window.parent === window) return;
    const nextPlace = placeOf(data);
    const nextSize = sizeOf(data);
    const nextVisible = (data as { visible?: unknown } | null)?.visible !== false;
    menu = (data as { menu?: unknown } | null)?.menu === true;
    pans = (data as { pans?: unknown } | null)?.pans === true;
    const token = (data as { measure?: unknown } | null)?.measure;
    const nextMeasuring = typeof token === 'string' && token ? token : null;
    last = '';
    const nextBoxed = inBox(nextPlace, nextSize);
    if (nextBoxed !== inBox(place, size)) holdStillInBox(nextBoxed);
    if (nextPlace !== place) {
      place = nextPlace;
      stamp(WIDGET_PLACE_ATTRIBUTE, place);
    }
    if (nextSize !== size) {
      size = nextSize;
      stamp(WIDGET_SIZE_ATTRIBUTE, size);
      notify(sizeListeners, size);
    }
    if (nextVisible !== visible) {
      visible = nextVisible;
      notify(visibilityListeners, visible);
    }
    if (nextMeasuring !== measuring) {
      measuring = nextMeasuring;
      if (measuring) void measureWhenSettled(measuring, size);
    }
    if (!installed) {
      installed = true;
      stopResources = watchWidgetResources(document, counts => postToHost({ type: WIDGET_RESOURCES_MESSAGE_TYPE, ...counts }));
      if (document.body) new ResizeObserver(schedule).observe(document.body);
      document.addEventListener('pointerdown', trackPress, { capture: true, passive: true });
      document.addEventListener('pointerdown', notePress, { capture: true, passive: true });
      document.addEventListener('pointerup', noteLift, { capture: true, passive: true });
      document.addEventListener('pointercancel', noteLift, { capture: true, passive: true });
      window.addEventListener('contextmenu', takeMenuPress, true);
      window.addEventListener('contextmenu', forwardContextMenu);
    }
    schedule();
  });
}

export const widget = {
  size(): WidgetSize | null {
    return size;
  },

  onSize(callback: (size: WidgetSize | null) => void): () => void {
    sizeListeners.add(callback);
    return () => sizeListeners.delete(callback);
  },

  place(): WidgetPlace | null {
    return place;
  },

  visible(): boolean {
    return visible;
  },

  onVisibility(callback: (visible: boolean) => void): () => void {
    visibilityListeners.add(callback);
    return () => visibilityListeners.delete(callback);
  },
};

export function _resetWidgetFrameForTesting(): void {
  installed = false;
  stopResources?.();
  stopResources = null;
  size = null;
  place = null;
  visible = true;
  measuring = null;
  menu = pans = menuPress = selectionAtPress = menuFromPointer = menuOverSelection = false;
  primaryDown = heldOpenedMenu = false;
  releaseNativeLongPress();
  holdStillInBox(false);
  endHold?.();
  sizeListeners.clear();
  visibilityListeners.clear();
  for (const attribute of [WIDGET_SIZE_ATTRIBUTE, WIDGET_PLACE_ATTRIBUTE]) {
    document.documentElement.removeAttribute(attribute);
  }
}
