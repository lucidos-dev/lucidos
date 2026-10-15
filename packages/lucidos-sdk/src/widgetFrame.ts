/**
 * A widget frame's half of the host contract (ADR 0402).
 *
 * A widget shows inline in a thread, so the host sizes its frame to the
 * content instead of filling a pane. Inline, the frame takes the full height
 * and never scrolls (ADR 0407). The host marks a widget frame with a
 * push on {@link WIDGET_CHANNEL} once it loads. Only then does this report,
 * so an app in the Canvas pane posts nothing.
 *
 * On a Home board tile, a tap posts {@link WIDGET_TAP_MESSAGE_TYPE}, so a
 * touch device can show the tile's menu.
 *
 * The push names the place the frame shows in and, on the Home board, the
 * tile's *widget size* (ADR 0432). Both are stamped on `<html>`, so most
 * widgets adapt in CSS alone, and `lucidos.widget` reads them. A tile the
 * board hides is told `visible: false` before it unloads, so it can stop its
 * timers.
 */

import { onHostPush } from './_bridge';
import { runIsolated } from './_isolate';
import { WIDGET_SIZES } from './generated/engine-constants';

/** The push channel the host marks a widget frame on. */
export const WIDGET_CHANNEL = 'widget';

/** What a widget frame posts when its content size changes. */
export const WIDGET_CONTENT_SIZE_MESSAGE_TYPE = 'lucidos:ui:size';

/** What a Home board tile's frame posts on a tap: a primary
 *  press that lifts before it travels past {@link PRESS_MOVE_TOLERANCE_PX}. */
export const WIDGET_TAP_MESSAGE_TYPE = 'lucidos:ui:widget-tap';

/** What a Home board tile's frame posts when a finger holds still on it, so
 *  the host can enter edit mode (ADR 0432). */
export const WIDGET_HOLD_MESSAGE_TYPE = 'lucidos:ui:widget-hold';

/** Where a still press stops being a tap, host and frame alike. */
export const LONG_PRESS_DELAY_MS = 450;

/** Pointer travel that cancels a press, so it is neither a tap nor a hold: a
 *  drag or a scroll. */
export const PRESS_MOVE_TOLERANCE_PX = 10;

/** The `<html>` attribute naming the tile's widget size on the Home board. */
export const WIDGET_SIZE_ATTRIBUTE = 'data-widget-size';

/** The `<html>` attribute naming where the widget shows. */
export const WIDGET_PLACE_ATTRIBUTE = 'data-widget-place';

/** Every place a widget frame shows in (ADRs 0407, 0415, 0432, 0466). A
 *  reply's embed is an inline widget, so it shows `inline`. */
export const WIDGET_PLACES = ['inline', 'shelf', 'option', 'home'] as const;

export type WidgetPlace = (typeof WIDGET_PLACES)[number];

/** One of the three sizes a widget takes as a Home board tile. */
export type WidgetSize = (typeof WIDGET_SIZES)[number];

/** What the host pushes on {@link WIDGET_CHANNEL}. Every field is optional,
 *  so an older host's push still reads. */
export interface WidgetPush {
  place?: WidgetPlace;
  size?: WidgetSize;
  visible?: boolean;
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

/** A Home board tile never scrolls inside (ADR 0432), whatever stylesheet the
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
const sizeListeners = new Set<(size: WidgetSize | null) => void>();
const visibilityListeners = new Set<(visible: boolean) => void>();

function notify<T>(listeners: Set<(value: T) => void>, value: T): void {
  for (const cb of [...listeners]) runIsolated(() => cb(value));
}

/** A press on a Home board tile. A lift that has not travelled past the
 *  tolerance is a tap, so a touch screen can show the tile's menu. A finger
 *  held still is a hold instead, which enters edit mode. A finger that scrolls
 *  the board gets a cancel, so it is neither. */
function trackPress(down: PointerEvent): void {
  if (place !== 'home' || !down.isPrimary) return;
  const end = () => {
    clearTimeout(timer);
    window.removeEventListener('pointermove', move, true);
    window.removeEventListener('pointerup', lift, true);
    window.removeEventListener('pointercancel', cancel, true);
  };
  const timer = down.pointerType !== 'mouse'
    ? setTimeout(() => {
      end();
      window.parent.postMessage({ type: WIDGET_HOLD_MESSAGE_TYPE }, '*');
    }, LONG_PRESS_DELAY_MS)
    : undefined;
  const move = (e: PointerEvent) => {
    if (e.pointerId !== down.pointerId) return;
    if (Math.hypot(e.screenX - down.screenX, e.screenY - down.screenY) > PRESS_MOVE_TOLERANCE_PX) end();
  };
  const lift = (e: PointerEvent) => {
    if (e.pointerId !== down.pointerId) return;
    end();
    window.parent.postMessage({ type: WIDGET_TAP_MESSAGE_TYPE, pointerType: down.pointerType }, '*');
  };
  const cancel = (e: PointerEvent) => {
    if (e.pointerId === down.pointerId) end();
  };
  window.addEventListener('pointermove', move, true);
  window.addEventListener('pointerup', lift, true);
  window.addEventListener('pointercancel', cancel, true);
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
    place = placeOf(data);
    const nextSize = sizeOf(data);
    const nextVisible = (data as { visible?: unknown } | null)?.visible !== false;
    // A new place can relay out the widget even at the same size, and the host
    // waits for a report there.
    last = '';
    stamp(WIDGET_PLACE_ATTRIBUTE, place);
    holdStillInTile(place === 'home');
    stamp(WIDGET_SIZE_ATTRIBUTE, nextSize);
    if (nextSize !== size) {
      size = nextSize;
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
    }
    schedule();
  });
}

/** `lucidos.widget`: what the host told this widget about where it shows. */
export const widget = {
  /** The tile's widget size on the Home board, `null` anywhere else. */
  size(): WidgetSize | null {
    return size;
  },

  /** Run `callback` each time the host gives the widget another size.
   *  Returns an unsubscribe function. */
  onSize(callback: (size: WidgetSize | null) => void): () => void {
    sizeListeners.add(callback);
    return () => sizeListeners.delete(callback);
  },

  /** Where the widget shows: `home`, `inline`, `shelf` or `option`.
   *  `null` outside a widget place, such as the Canvas. */
  place(): WidgetPlace | null {
    return place;
  },

  /** False while the host keeps the frame loaded but out of sight, as a
   *  hidden Home board does before it unloads. */
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
  sizeListeners.clear();
  visibilityListeners.clear();
  for (const attribute of [WIDGET_SIZE_ATTRIBUTE, WIDGET_PLACE_ATTRIBUTE]) {
    document.documentElement.removeAttribute(attribute);
  }
}
