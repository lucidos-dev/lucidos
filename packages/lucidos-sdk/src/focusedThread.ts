
import { FOCUSED_THREAD_CHANNEL, callHost, onHostPush } from './_bridge';

function asThreadId(value: unknown): string | null | undefined {
  if (value === null) return null;
  if (typeof value === 'string' && value.length > 0) return value;
  return undefined;
}

export async function focusedThread(): Promise<string | null> {
  if (window.parent === window) {
    throw new Error('lucidos.ui.focusedThread: no host to ask (this app is not running inside Lucidos)');
  }
  const answer = asThreadId(await callHost('ui.focused-thread', null));
  if (answer === undefined) {
    throw new Error('lucidos.ui.focusedThread: the host answered with no thread id');
  }
  return answer;
}

export function onFocusedThreadChange(callback: (threadId: string | null) => void): () => void {
  if (typeof callback !== 'function') {
    throw new TypeError('lucidos.ui.onFocusedThreadChange: callback must be a function');
  }
  return onHostPush(FOCUSED_THREAD_CHANNEL, (data) => {
    const threadId = asThreadId((data as { threadId?: unknown } | null)?.threadId);
    if (threadId === undefined) {
      console.warn('[lucidos-sdk] dropped a focused-thread push with no thread id:', data);
      return;
    }
    callback(threadId);
  });
}
