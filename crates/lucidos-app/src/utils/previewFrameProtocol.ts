
export const PREVIEW_FRAME_ROLE = 'artifact-preview-frame';

export const PREVIEW_FRAME_MESSAGE = 'lucidos:preview-frame';

export const PREVIEW_HOST_MESSAGE = 'lucidos:preview-host';

export type PreviewFindArgs = { query: string; step?: 1 | -1 } | { clear: true };

export type PreviewHostMessage =
  | { kind: 'scroll'; id: string; smooth: boolean }
  | { kind: 'capability'; capability: string }
  | { kind: 'find'; id: string; args: PreviewFindArgs };

export function postToPreviewFrame(frameWindow: Window | null, message: PreviewHostMessage): void {
  frameWindow?.postMessage({ type: PREVIEW_HOST_MESSAGE, ...message }, '*');
}

const pendingFinds = new Map<string, (reply: unknown) => void>();
let findCounter = 0;

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

export function settlePreviewFind(id: string, reply: unknown): void {
  const settle = pendingFinds.get(id);
  if (!settle) return;
  pendingFinds.delete(id);
  settle(reply);
}
