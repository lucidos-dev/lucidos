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

/** What a find request asks the frame's finder to do. */
export type PreviewFindArgs = { query: string; step?: 1 | -1 } | { clear: true };

/** What the host tells the frame. */
export type PreviewHostMessage =
  /** Scroll to an in-document anchor, or to the top for `''`. */
  | { kind: 'scroll'; id: string; smooth: boolean }
  /** A renewed asset pass, to swap into the frame's `<base href>`. */
  | { kind: 'capability'; capability: string }
  /** Run the find bar's query. An empty `id` wants no answer. */
  | { kind: 'find'; id: string; args: PreviewFindArgs };

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

/** Find requests waiting for their frame's answer, by request id. */
const pendingFinds = new Map<string, (reply: unknown) => void>();
let findCounter = 0;

/**
 * Ask a preview frame to run a find, and wait for its answer.
 *
 * The answer arrives as a `find-result` message, which the bridge's router
 * accepts only under the render's nonce and hands to `settlePreviewFind`. It
 * resolves to the frame's raw count, or null when the frame's finder failed.
 * The caller validates the count: it came from code the host does not trust.
 */
export function askPreviewFind(frameWindow: Window, args: PreviewFindArgs, timeoutMs: number): Promise<unknown> {
  const id = `find-${++findCounter}`;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pendingFinds.delete(id);
      reject(new Error(`the preview did not answer a find in ${timeoutMs}ms`));
    }, timeoutMs);
    pendingFinds.set(id, (reply) => {
      clearTimeout(timer);
      resolve(reply);
    });
    postToPreviewFrame(frameWindow, { kind: 'find', id, args });
  });
}

/** A checked `find-result` from a preview frame. An unknown id is dropped. */
export function settlePreviewFind(id: string, reply: unknown): void {
  const settle = pendingFinds.get(id);
  if (!settle) return;
  pendingFinds.delete(id);
  settle(reply);
}
