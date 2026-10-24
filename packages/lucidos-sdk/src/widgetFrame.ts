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

import { onHostPush } from './_bridge';
import { hapticTick } from './haptic';
import { opensContextMenu, selectionCovers, shouldSuppressContextMenu, textAt, type Point } from './contextMenu';
import { runIsolated } from './_isolate';
import { WIDGET_SIZES } from './generated/engine-constants';

/** The push channel the host marks a widget frame on. */
export const WIDGET_CHANNEL = 'widget';

/** What a widget frame posts when its content size changes. */
export const WIDGET_CONTENT_SIZE_MESSAGE_TYPE = 'lucidos:ui:size';

/** What a frame marked `menu` posts to open the widget's menu: on a
 *  right-click, and on a finger held still on a dashboard tile. */
export const WIDGET_MENU_MESSAGE_TYPE = 'lucidos:ui:widget-menu';

/** A {@link WIDGET_MENU_MESSAGE_TYPE} message. `at` is where a pointer pressed,
 *  in the frame's own viewport. A hold or a keyboard-raised menu has none.
 *  `held` says a press has not delivered its click yet: a finger still down, or
 *  a hold opening the menu at its lift. That click may reach the host once the
 *  menu inerts the frame, and the menu must spend it. */
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
export const WIDGET_PLACES = ['inline', 'shelf', 'option', 'dashboard'] as const;

export type WidgetPlace = (typeof WIDGET_PLACES)[number];

/** One of the three sizes a widget takes as a dashboard tile. */
export type WidgetSize = (typeof WIDGET_SIZES)[number];

/** What the host pushes on {@link WIDGET_CHANNEL}. Every field is optional,
 *  so an older host's push still reads. */
export interface WidgetPush {
  place?: WidgetPlace;
  size?: WidgetSize;
  visible?: boolean;
  /** The host opens a widget menu for this frame, so a right-click goes there. */
  menu?: boolean;
}

export interface ContentSize {
  height: number;
}

function verticalMargins(doc: Document): number {
  const style = doc.defaultView?.getComputedStyle(doc.body);
  return style ? (parseFloat(style.marginTop) || 0) + (parseFloat(style.marginBottom) || 0) : 0;
}

/** The content's height in CSS pixels, margins included. A body that fills
 *  the viewport reports the viewport, so a widget must not size its body to
 *  `100vh` or the frame can never shrink. */
export function contentHeight(doc: Document): number {
  if (!doc.body) return 0;
  return Math.ceil(doc.body.scrollHeight + verticalMargins(doc));
}

function sizeOf(data: unknown): WidgetSize | null {
  const size = (data as { size?: unknown } | null)?.size;
  return WIDGET_SIZES.includes(size as WidgetSize) ? (size as WidgetSize) : null;
}

function placeOf(data: unknown): WidgetPlace | null {
  const place = (data as { place?: unknown } | null)?.place;
  return WIDGET_PLACES.includes(place as WidgetPlace) ? (place as WidgetPlace) : null;
}

/** A dashboard tile never scrolls inside (ADR 0432), whatever stylesheet the
 *  widget links. Inline and constant, so no stylesheet is built (ADR 0307). */
function holdStillInTile(inTile: boolean): void {
  for (const el of [document.documentElement, document.body]) {
    if (!el) continue;
    if (inTile) el.style.setProperty('overflow', 'hidden', 'important');
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
const sizeListeners = new Set<(size: WidgetSize | null) => void>();
const visibilityListeners = new Set<(visible: boolean) => void>();

function notify<T>(listeners: Set<(value: T) => void>, value: T): void {
  for (const cb of [...listeners]) runIsolated(() => cb(value));
}

/** iOS answers a resting finger with its own selection or link menu, while
 *  ours waits for the lift. A held press turns both off until the next press,
 *  so the menu comes alone. */
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
  window.parent.postMessage(request, '*');
}

/** A finger held still on a dashboard tile marked `menu` opens the menu at its
 *  lift, with a haptic tick. A finger that lifts early, scrolls the dashboard
 *  or travels past the tolerance cancels the hold.
 *
 *  The hold is read from the events' own times, never a timer. iOS can take a
 *  gesture and send no lift, and a timer reads that silence as a hold. The
 *  press turns off iOS's own long-press answer at once, since iOS would show
 *  it before the lift. The next press gives it back. A new press first ends
 *  one whose lift was lost. */
function trackPress(down: PointerEvent): void {
  endHold?.();
  releaseNativeLongPress();
  if (place !== 'dashboard' || !down.isPrimary || down.pointerType === 'mouse' || !menu) return;
  const end = () => {
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

/** Start reporting once the host marks this frame a widget. A later push
 *  updates the place and size and reports again. */
export function installWidgetFrame(): void {
  let last = '';
  let scheduled = false;
  const report = () => {
    scheduled = false;
    const size: ContentSize = { height: contentHeight(document) };
    const key = String(size.height);
    if (key === last) return;
    last = key;
    window.parent.postMessage({ type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, ...size }, '*');
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
    // A new place can relay out the widget even at the same size, and the host
    // waits for a report there.
    last = '';
    // Write `<html>` only when the host's value changes. Rewriting a value
    // still queues a mutation record. A widget observer that answers each
    // record with a write of its own would then loop forever.
    if (nextPlace !== place) {
      if ((nextPlace === 'dashboard') !== (place === 'dashboard')) holdStillInTile(nextPlace === 'dashboard');
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
  /** The tile's widget size on the dashboard, `null` anywhere else. */
  size(): WidgetSize | null {
    return size;
  },

  /** Run `callback` each time the host gives the widget another size.
   *  Returns an unsubscribe function. */
  onSize(callback: (size: WidgetSize | null) => void): () => void {
    sizeListeners.add(callback);
    return () => sizeListeners.delete(callback);
  },

  /** Where the widget shows: `dashboard`, `inline`, `shelf` or `option`.
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
  menu = menuPress = selectionAtPress = menuFromPointer = menuOverSelection = false;
  primaryDown = heldOpenedMenu = false;
  releaseNativeLongPress();
  endHold?.();
  sizeListeners.clear();
  visibilityListeners.clear();
  for (const attribute of [WIDGET_SIZE_ATTRIBUTE, WIDGET_PLACE_ATTRIBUTE]) {
    document.documentElement.removeAttribute(attribute);
  }
}
