/**
 * The wire names the HTML artifact preview frame and the host share.
 *
 * The frame runs at an opaque origin (ADR 0322), so the two talk only over
 * `postMessage`. Split out from `components/files/previewFrameBridge.ts`
 * because the store's pass renewal posts into the frame too, and a store
 * module should not import a component.
 */

/** `data-role` on every mounted preview frame. The pass renewal finds them by it. */
export const PREVIEW_FRAME_ROLE = 'artifact-preview-frame';

/** `type` on a message the frame posts up. */
export const PREVIEW_FRAME_MESSAGE = 'lucidos:preview-frame';

/** `type` on a message the host posts down. */
export const PREVIEW_HOST_MESSAGE = 'lucidos:preview-host';

/** What the host tells the frame. */
export type PreviewHostMessage =
  /** Scroll to an in-document anchor, or to the top for `''`. */
  | { kind: 'scroll'; id: string; smooth: boolean }
  /** A renewed asset pass, to swap into the frame's `<base href>`. */
  | { kind: 'capability'; capability: string };

/**
 * Post one message down into a preview frame.
 *
 * The target origin is `*` because an opaque origin cannot be named any other
 * way. Neither payload is a secret from the frame: a scroll target, and a pass
 * for the tree it already holds a pass to.
 */
export function postToPreviewFrame(frameWindow: Window | null, message: PreviewHostMessage): void {
  frameWindow?.postMessage({ type: PREVIEW_HOST_MESSAGE, ...message }, '*');
}
