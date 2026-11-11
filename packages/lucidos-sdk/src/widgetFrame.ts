/**
 * A widget frame's half of the host contract (ADR 0402).
 *
 * A widget shows inline in a thread, so the host sizes its frame to the
 * content instead of filling a pane. Inline, the frame takes the full height
 * and never scrolls (ADR 0407). The host marks a widget frame with a
 * push on {@link WIDGET_CHANNEL} once it loads. Only then does this report,
 * so an app in the Canvas pane posts nothing.
 *
 * A frame the host marks with `menu` forwards an unclaimed right-click as
 * {@link WIDGET_MENU_MESSAGE_TYPE}, so the host opens the widget's own menu.
 * Content keeps the native menu by the host's own rule (`contextMenu.ts`).
 *
 * The push names the place the frame shows in and, on the dashboard, the
 * tile's *widget size* (ADR 0432). Both are stamped on `<html>`, so most
 * widgets adapt in CSS alone, and `lucidos.widget` reads them. A tile the
 * dashboard hides is told `visible: false` before it unloads, so it can stop its
 * timers.
 */

import { onHostPush, postToHost } from './_bridge';
import { hapticTick } from './haptic';
import { opensContextMenu, selectionCovers, shouldSuppressContextMenu, textAt, type Point } from './contextMenu';
import { runIsolated } from './_isolate';
import { WIDGET_SIZES } from './generated/engine-constants';
import { domGeometry, measureFit, settle, type FitResult } from './widgetFit';

export { FIT_EDGES, FIT_KINDS, MAX_FIT_FINDINGS, SETTLE_CAP_MS, type FitFinding } from './widgetFit';

/** The push channel the host marks a widget frame on. */
export const WIDGET_CHANNEL = 'widget';

/** What a widget frame posts when its content size changes. */
export const WIDGET_CONTENT_SIZE_MESSAGE_TYPE = 'lucidos:ui:size';

/** What a frame marked `menu` posts to open the widget's menu: on a
 *  right-click, and on a finger held still on a thread card or a dashboard
 *  tile. */
export const WIDGET_MENU_MESSAGE_TYPE = 'lucidos:ui:widget-menu';

/** What a frame posts once it has measured for a push's `measure` token. */
export const WIDGET_FIT_MESSAGE_TYPE = 'lucidos:ui:widget-fit';

/** A {@link WIDGET_MENU_MESSAGE_TYPE} message. `at` is where a pointer pressed,
 *  in the frame's own viewport. A hold or a keyboard-raised menu has none.
 *  `held` says a press has not delivered its click yet: a finger still down, or
 *  a hold a busy frame read only at its lift. That click may reach the host
 *  once the menu inerts the frame, and the menu must spend it. */
export interface WidgetMenuRequest {
  type: typeof WIDGET_MENU_MESSAGE_TYPE;
  at?: Point;
  held?: true;
}

/** Where a still press stops being a tap, host and frame alike. */
export const LONG_PRESS_DELAY_MS = 450;

/** Pointer travel that cancels a hold: a drag or a scroll. */
export const PRESS_MOVE_TOLERANCE_PX = 10;

/** The `<html>` attribute naming the tile's widget size on the dashboard. */
export const WIDGET_SIZE_ATTRIBUTE = 'data-widget-size';

/** The `<html>` attribute naming where the widget shows. */
export const WIDGET_PLACE_ATTRIBUTE = 'data-widget-place';

/** Every place a widget frame shows in (ADRs 0407, 0415, 0432, 0466). A
 *  reply's embed is an inline widget, so it shows `inline`. */
export const WIDGET_PLACES = ['inline', 'option', 'dashboard'] as const;

export type WidgetPlace = (typeof WIDGET_PLACES)[number];

/** One of the sizes a widget takes as a thread card or a dashboard tile. */
export type WidgetSize = (typeof WIDGET_SIZES)[number];

/** What the host pushes on {@link WIDGET_CHANNEL}. Every field is optional,
 *  so an older host's push still reads. */
export interface WidgetPush {
  place?: WidgetPlace;
  size?: WidgetSize;
  visible?: boolean;
  /** The host opens a widget menu for this frame, so a right-click goes there. */
  menu?: boolean;
  /** The host clamped the box below its true width and scrolls it sideways. */
  pans?: boolean;
  /** The *widget fit check* asks this frame to settle and report what its box
   *  cuts off. Each size it walks carries a new token. */
  measure?: string;
}

/** A content-size report. `size` is the widget size the widget laid out
 *  for, so the host can tell a report from before a resize from one after. */
export interface ContentSize {
  /** At least the box's width. More means the content spills sideways. */
  width?: number;
  height: number;
  size?: WidgetSize;
}

/** A {@link WIDGET_FIT_MESSAGE_TYPE} message: the box the frame measured and
 *  what it cuts off, or why it could not measure. */
export type WidgetFitReport = { type: typeof WIDGET_FIT_MESSAGE_TYPE; token: string; size?: WidgetSize }
  & (({ box: { width: number; height: number } } & FitResult) | { error: string });

function margins(doc: Document, start: 'marginTop' | 'marginLeft', end: 'marginBottom' | 'marginRight'): number {
  const style = doc.defaultView?.getComputedStyle(doc.body);
  return style ? (parseFloat(style[start]) || 0) + (parseFloat(style[end]) || 0) : 0;
}

/** The content's height in CSS pixels, margins included. A body that fills
 *  the viewport reports the viewport, so a widget must not size its body to
 *  `100vh` or the frame can never shrink. */
export function contentHeight(doc: Document): number {
  if (!doc.body) return 0;
  return Math.ceil(doc.body.scrollHeight + margins(doc, 'marginTop', 'marginBottom'));
}

/** The content's width in CSS pixels, side margins included. In a box,
 *  `holdStillInBox` makes the body a scroll container, so its `scrollWidth`
 *  counts what spills past. */
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

/** A *widget box*: a thread card or a dashboard tile given a size. The host
 *  sets its box, and an option or a place with no size has none. */
function inBox(at: WidgetPlace | null, sized: WidgetSize | null): boolean {
  return sized !== null && (at === 'inline' || at === 'dashboard');
}

/** A widget box never scrolls inside (ADR 0432), whatever stylesheet the
 *  widget links. Inline and constant, so no stylesheet is built (ADR 0307). */
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
let size: WidgetSize | null = null;
let place: WidgetPlace | null = null;
let visible = true;
let menu = false;
let pans = false;
// The fit check's token for the current push, else null.
let measuring: string | null = null;
// The press that may raise the next `contextmenu`, taken in its capture phase.
let menuPress = false;
let selectionAtPress = false;
// What the capture phase took for the `contextmenu` now dispatching.
let menuFromPointer = false;
let menuOverSelection = false;
// A primary press that has not lifted yet.
let primaryDown = false;
// A held finger's press, which a `contextmenu` arriving first cancels.
let endHold: (() => void) | null = null;
// Set once a hold opened the menu: the platform's own long-press menu that
// may follow it is swallowed, so one press opens one menu.
let heldOpenedMenu = false;
// The root's own selection styles, kept from a hold until the next press.
let stylesBeforeHold: Array<[property: string, value: string, priority: string]> | null = null;
/** A box the host clamped below its true width, which only an XL box can be,
 *  scrolls sideways (ADR 0633). A sideways drag in it belongs to that scroller. */
export function inSidewaysBox(): boolean {
  return pans && inBox(place, size);
}

const sizeListeners = new Set<(size: WidgetSize | null) => void>();
const visibilityListeners = new Set<(visible: boolean) => void>();

function notify<T>(listeners: Set<(value: T) => void>, value: T): void {
  for (const cb of [...listeners]) runIsolated(() => cb(value));
}

/** iOS answers a resting finger with its own selection or link menu, while
 *  ours opens. A held press turns both off until the next press, so the menu
 *  comes alone. */
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

/** A finger held still on a thread card or a dashboard tile marked `menu`
 *  opens the menu while it is still down. A haptic tick marks it. A finger that
 *  lifts early, scrolls or travels past the tolerance cancels the hold. An
 *  option frame never holds: it has no widget menu.
 *
 *  The hold is read from the events' own times as well as a timer. A frame
 *  busy through the whole press gets the lift before its timer can run. The
 *  press turns off iOS's own long-press answer at once, and the next press
 *  gives it back. A new press first ends one whose lift was lost. */
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

/** Capture phase, so a widget that stops propagation cannot hide its press.
 *  Only a menu press measures the selection, as the host's policy does. */
function notePress(e: PointerEvent): void {
  heldOpenedMenu = false;
  primaryDown = e.isPrimary !== false && e.button === 0;
  menuPress = opensContextMenu(e);
  selectionAtPress = menuPress && selectionCovers(window.getSelection(), { x: e.clientX, y: e.clientY });
}

function noteLift(e: PointerEvent): void {
  if (e.isPrimary !== false) primaryDown = false;
}

/** In the capture phase, which no widget handler can stop, so a press is
 *  spent by its own `contextmenu` even when the widget claims it. */
function takeMenuPress(): void {
  menuFromPointer = menuPress;
  menuOverSelection = selectionAtPress;
  menuPress = selectionAtPress = false;
  endHold?.();
}

/** On `window` in the bubble phase, so the widget's own handlers on an
 *  element or the document run first and can claim the event. */
function forwardContextMenu(e: MouseEvent): void {
  // A button down that this frame never saw pressed means a pointer raised it
  // elsewhere: the press that closed the host's open menu never reaches here.
  // A finger this frame saw land stays a hold.
  const fromPointer = menuFromPointer || (!primaryDown && e.buttons !== 0);
  const heldSelection = menuOverSelection;
  if (heldOpenedMenu) {
    heldOpenedMenu = false;
    e.preventDefault();
    return;
  }
  if (!menu) return;
  const point = { x: e.clientX, y: e.clientY };
  // A menu raised by the keyboard had no press, and its point means nothing.
  const textUnderPointer = () => !fromPointer || textAt(e.target, point);
  if (!shouldSuppressContextMenu(e, heldSelection, textUnderPointer)) return;
  e.preventDefault();
  // Undo the word WebKit selected on the way in. The press found no
  // selection under the pointer, so one there now is WebKit's.
  const selection = window.getSelection();
  if (selectionCovers(selection, point)) selection?.removeAllRanges();
  postMenuRequest(fromPointer ? point : undefined);
}

/** Settle, then report for `token` unless a later push replaced it. The size
 *  is the one the frame laid out for when the token came. */
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

/** Start reporting once the host marks this frame a widget. A later push
 *  updates the place and size and reports again. */
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
    // A new place can relay out the widget even at the same size, and the host
    // waits for a report there.
    last = '';
    // Write `<html>` only when the host's value changes. Rewriting a value
    // still queues a mutation record. A widget observer that answers each
    // record with a write of its own would then loop forever.
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

/** `lucidos.widget`: what the host told this widget about where it shows. */
export const widget = {
  /** The widget box's size, in a thread card or a dashboard tile. `null`
   *  anywhere else, such as a question option or the Canvas. */
  size(): WidgetSize | null {
    return size;
  },

  /** Run `callback` each time the host gives the widget another size.
   *  Returns an unsubscribe function. */
  onSize(callback: (size: WidgetSize | null) => void): () => void {
    sizeListeners.add(callback);
    return () => sizeListeners.delete(callback);
  },

  /** Where the widget shows: `dashboard`, `inline` or `option`.
   *  `null` outside a widget place, such as the Canvas. */
  place(): WidgetPlace | null {
    return place;
  },

  /** False while the host keeps the frame loaded but out of sight, as a
   *  hidden dashboard does before it unloads. */
  visible(): boolean {
    return visible;
  },

  /** Run `callback` each time the widget is hidden or shown again. Stop
   *  timers on `false`, and re-read on `true`. Returns an unsubscribe. */
  onVisibility(callback: (visible: boolean) => void): () => void {
    visibilityListeners.add(callback);
    return () => visibilityListeners.delete(callback);
  },
};

/** Test-only: forget the one-shot install, so a case can set up a fresh frame. */
export function _resetWidgetFrameForTesting(): void {
  installed = false;
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
