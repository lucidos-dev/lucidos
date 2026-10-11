/**
 * A widget frame's half of the host contract (ADRs 0402, 0419).
 *
 * A widget shows inline in a thread, so the host sizes its frame to the
 * content instead of filling a pane. Inline, the frame takes the full height
 * and never scrolls (ADR 0407). The host marks a widget frame with a
 * push on {@link WIDGET_CHANNEL} once it loads. Only then does this report,
 * so an app in the Canvas pane posts nothing.
 *
 * A widget window adds its mode to the push. This stamps it on `<html>` as
 * `data-widget-mode`, so the widget's CSS can draw a compact form. In
 * `minimal` mode the report also carries the content's width. In any window
 * mode a press posts {@link WIDGET_TAP_MESSAGE_TYPE}, so a touch device can
 * show the window's controls.
 *
 * The push also says whether the window can move. If it can, a mouse or pen
 * drag that starts on no control posts {@link WIDGET_DRAG_MESSAGE_TYPE}, and
 * the host moves the window with it.
 */

import { onHostPush } from './_bridge';

/** The push channel the host marks a widget frame on. */
export const WIDGET_CHANNEL = 'widget';

/** What a widget frame posts when its content size changes. */
export const WIDGET_SIZE_MESSAGE_TYPE = 'lucidos:ui:size';

/** What a frame in a widget window posts on a primary press. */
export const WIDGET_TAP_MESSAGE_TYPE = 'lucidos:ui:widget-tap';

/** What a frame in a movable widget window posts while it is dragged. */
export const WIDGET_DRAG_MESSAGE_TYPE = 'lucidos:ui:widget-drag';

/** Pointer travel before a press becomes a drag, so a click stays a click. */
export const WIDGET_DRAG_THRESHOLD_PX = 4;

/** A drag's steps, in order. */
export const WIDGET_DRAG_PHASES = ['start', 'move', 'end'] as const;

export type WidgetDragPhase = (typeof WIDGET_DRAG_PHASES)[number];

/** A drag's step, in screen pixels: the frame moves with the window, so only
 *  screen coordinates hold still under the pointer. */
export interface WidgetDrag {
  phase: WidgetDragPhase;
  screenX: number;
  screenY: number;
}

/** The window mode that shrink-wraps the widget, so it reports its width. */
export const MINIMAL_WIDGET_MODE = 'minimal';

/** The `<html>` attribute naming the widget window's mode. */
export const WIDGET_MODE_ATTRIBUTE = 'data-widget-mode';

export interface ContentSize {
  height: number;
  /** Only in minimal mode. */
  width?: number;
}

function margins(doc: Document, a: 'Top' | 'Left', b: 'Bottom' | 'Right'): number {
  const style = doc.defaultView?.getComputedStyle(doc.body);
  return style ? (parseFloat(style[`margin${a}`]) || 0) + (parseFloat(style[`margin${b}`]) || 0) : 0;
}

/** The content's height in CSS pixels, margins included. A body that fills
 *  the viewport reports the viewport, so a widget must not size its body to
 *  `100vh` or the frame can never shrink. */
export function contentHeight(doc: Document): number {
  if (!doc.body) return 0;
  return Math.ceil(doc.body.scrollHeight + margins(doc, 'Top', 'Bottom'));
}

/** The content's width in CSS pixels, margins included. Minimal mode sizes
 *  the body to its content (`sdk_iframe.css`), so this is the widget's shape. */
export function contentWidth(doc: Document): number {
  if (!doc.body) return 0;
  return Math.ceil(doc.body.getBoundingClientRect().width + margins(doc, 'Left', 'Right'));
}

function modeOf(data: unknown): string | null {
  const mode = (data as { mode?: unknown } | null)?.mode;
  return typeof mode === 'string' ? mode : null;
}

/** Everything a press may mean to the widget itself. A drag never starts on
 *  one, so the widget keeps every click, field and slider. */
const CONTROLS = [
  'a[href]', 'button', 'input', 'select', 'textarea', 'label', 'summary',
  'audio', 'video', 'iframe', '[draggable="true"]',
  '[contenteditable]:not([contenteditable="false"])',
  ...['button', 'link', 'checkbox', 'radio', 'switch', 'slider', 'tab', 'menuitem',
    'option', 'textbox', 'combobox', 'spinbutton', 'scrollbar'].map((role) => `[role="${role}"]`),
].join(', ');

/** Whether a press lands where nothing in the widget answers it. A pointer
 *  cursor marks something clickable, and a handled press is the widget's. */
export function pressIsFree(e: PointerEvent): boolean {
  if (e.defaultPrevented || !e.isPrimary || e.button !== 0 || e.pointerType === 'touch') return false;
  const target = e.target;
  if (!(target instanceof Element) || target.closest(CONTROLS)) return false;
  // A press past the content box lands on the element's own scrollbar.
  if ((target.clientWidth > 0 && e.offsetX > target.clientWidth)
    || (target.clientHeight > 0 && e.offsetY > target.clientHeight)) return false;
  return getComputedStyle(target).cursor !== 'pointer';
}

let installed = false;
let mode: string | null = null;
let draggable = false;

/** Follows a free press. Past the threshold it captures the pointer, stops
 *  text selection and posts each step of the drag to the host. */
function trackDrag(down: PointerEvent): void {
  let dragging = false;
  // Where the pointer last really was. A cancel's own coordinates can be
  // anything, often zero, so a cancelled drag ends here instead.
  let last = { screenX: down.screenX, screenY: down.screenY };
  const post = (phase: WidgetDragPhase, at: { screenX: number; screenY: number }) => {
    const drag: WidgetDrag = { phase, screenX: at.screenX, screenY: at.screenY };
    window.parent.postMessage({ type: WIDGET_DRAG_MESSAGE_TYPE, ...drag }, '*');
  };
  const noNativeDrag = (e: Event) => e.preventDefault();
  const move = (e: PointerEvent) => {
    if (e.pointerId !== down.pointerId) return;
    // A release outside the frame never reached it, so a move with no button
    // held means the press is over.
    if ((e.buttons & 1) === 0) {
      end(e);
      return;
    }
    if (!dragging) {
      if (Math.hypot(e.screenX - down.screenX, e.screenY - down.screenY) < WIDGET_DRAG_THRESHOLD_PX) return;
      dragging = true;
      // The root stays in the document, where a widget may have redrawn the
      // pressed element since.
      document.documentElement.setPointerCapture?.(e.pointerId);
      post('start', down);
    }
    window.getSelection()?.removeAllRanges();
    last = { screenX: e.screenX, screenY: e.screenY };
    post('move', last);
  };
  const end = (e: PointerEvent) => {
    if (e.pointerId !== down.pointerId) return;
    window.removeEventListener('pointermove', move, true);
    window.removeEventListener('pointerup', end, true);
    window.removeEventListener('pointercancel', end, true);
    window.removeEventListener('dragstart', noNativeDrag, true);
    if (dragging) post('end', e.type === 'pointercancel' ? last : e);
  };
  window.addEventListener('pointermove', move, true);
  window.addEventListener('pointerup', end, true);
  window.addEventListener('pointercancel', end, true);
  // An image would otherwise start the browser's own drag and cancel ours.
  window.addEventListener('dragstart', noNativeDrag, true);
}

function postTap(e: PointerEvent): void {
  if (mode && e.isPrimary) window.parent.postMessage({ type: WIDGET_TAP_MESSAGE_TYPE, pointerType: e.pointerType }, '*');
}

function dragFromFreePress(e: PointerEvent): void {
  if (draggable && pressIsFree(e)) trackDrag(e);
}

/** Start reporting once the host marks this frame a widget. A later push
 *  changes the mode and reports again. */
export function installWidgetFrame(): void {
  let last = '';
  let scheduled = false;
  const report = () => {
    scheduled = false;
    const size: ContentSize = { height: contentHeight(document) };
    if (mode === MINIMAL_WIDGET_MODE) size.width = contentWidth(document);
    const key = `${size.height}x${size.width ?? ''}`;
    if (key === last) return;
    last = key;
    window.parent.postMessage({ type: WIDGET_SIZE_MESSAGE_TYPE, ...size }, '*');
  };
  const schedule = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(report);
  };
  onHostPush(WIDGET_CHANNEL, (data) => {
    if (window.parent === window) return;
    mode = modeOf(data);
    draggable = (data as { draggable?: unknown } | null)?.draggable === true;
    // A mode change relays out the widget even at the same size, and the host
    // waits for a report in the new mode.
    last = '';
    if (mode) document.documentElement.setAttribute(WIDGET_MODE_ATTRIBUTE, mode);
    else document.documentElement.removeAttribute(WIDGET_MODE_ATTRIBUTE);
    if (!installed) {
      installed = true;
      if (document.body) new ResizeObserver(schedule).observe(document.body);
      document.addEventListener('pointerdown', postTap, { capture: true, passive: true });
      // Bubble phase on the window, so the widget's own handlers run first and
      // a press they claim with preventDefault stays theirs.
      window.addEventListener('pointerdown', dragFromFreePress);
    }
    schedule();
  });
}

/** Test-only: forget the one-shot install, so a case can set up a fresh frame. */
export function _resetWidgetFrameForTesting(): void {
  installed = false;
  mode = null;
  draggable = false;
  document.documentElement.removeAttribute(WIDGET_MODE_ATTRIBUTE);
}
