/**
 * A widget frame tells the host how tall its content is (ADR 0402).
 *
 * A widget shows inline in a thread, so the host sizes its frame to the
 * content instead of filling a pane. Inline, the frame takes the full height
 * and never scrolls (ADR 0407). The host marks a widget frame with a
 * push on {@link WIDGET_CHANNEL} once it loads. Only then does this report,
 * so an app in the Canvas pane posts nothing.
 */

import { onHostPush } from './_bridge';

/** The push channel the host marks a widget frame on. */
export const WIDGET_CHANNEL = 'widget';

/** What a widget frame posts when its content height changes. */
export const WIDGET_HEIGHT_MESSAGE_TYPE = 'lucidos:ui:height';

/** The content's height in CSS pixels, margins included. A body that fills
 *  the viewport reports the viewport, so a widget must not size its body to
 *  `100vh` or the frame can never shrink. */
export function contentHeight(doc: Document): number {
  const body = doc.body;
  if (!body) return 0;
  const style = doc.defaultView?.getComputedStyle(body);
  const margins = style ? (parseFloat(style.marginTop) || 0) + (parseFloat(style.marginBottom) || 0) : 0;
  return Math.ceil(body.scrollHeight + margins);
}

let reporting = false;

/** Start reporting the height once the host marks this frame a widget. */
export function installWidgetHeight(): void {
  onHostPush(WIDGET_CHANNEL, () => {
    if (reporting || window.parent === window) return;
    reporting = true;
    let last = -1;
    let scheduled = false;
    const report = () => {
      scheduled = false;
      const height = contentHeight(document);
      if (height === last) return;
      last = height;
      window.parent.postMessage({ type: WIDGET_HEIGHT_MESSAGE_TYPE, height }, '*');
    };
    const schedule = () => {
      if (scheduled) return;
      scheduled = true;
      requestAnimationFrame(report);
    };
    if (document.body) new ResizeObserver(schedule).observe(document.body);
    schedule();
  });
}

/** Test-only: forget the one-shot install, so a case can set up a fresh frame. */
export function _resetWidgetHeightForTesting(): void {
  reporting = false;
}
