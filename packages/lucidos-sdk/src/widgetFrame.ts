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
 */

import { onHostPush } from './_bridge';

/** The push channel the host marks a widget frame on. */
export const WIDGET_CHANNEL = 'widget';

/** What a widget frame posts when its content size changes. */
export const WIDGET_SIZE_MESSAGE_TYPE = 'lucidos:ui:size';

/** What a frame in a widget window posts on a primary press. */
export const WIDGET_TAP_MESSAGE_TYPE = 'lucidos:ui:widget-tap';

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

let installed = false;
let mode: string | null = null;

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
    // A mode change relays out the widget even at the same size, and the host
    // waits for a report in the new mode.
    last = '';
    if (mode) document.documentElement.setAttribute(WIDGET_MODE_ATTRIBUTE, mode);
    else document.documentElement.removeAttribute(WIDGET_MODE_ATTRIBUTE);
    if (!installed) {
      installed = true;
      if (document.body) new ResizeObserver(schedule).observe(document.body);
      document.addEventListener('pointerdown', (e) => {
        if (mode && e.isPrimary) window.parent.postMessage({ type: WIDGET_TAP_MESSAGE_TYPE, pointerType: e.pointerType }, '*');
      }, { capture: true, passive: true });
    }
    schedule();
  });
}

/** Test-only: forget the one-shot install, so a case can set up a fresh frame. */
export function _resetWidgetFrameForTesting(): void {
  installed = false;
  mode = null;
  document.documentElement.removeAttribute(WIDGET_MODE_ATTRIBUTE);
}
