
import { isTauri } from './platform';
import { getNativeWindowActive, getNativeWindowShown, listen } from './tauri';

let nativeWindowActive = true;

export function isNativeWindowActive(): boolean {
  return nativeWindowActive;
}

export function setNativeWindowActive(active: boolean): void {
  nativeWindowActive = active;
}

export async function startNativeWindowActiveTracking(
  onChange?: (active: boolean) => void,
): Promise<() => void> {
  if (!isTauri()) return () => {};
  try {
    const active = await getNativeWindowActive();
    setNativeWindowActive(active);
    onChange?.(active);
  } catch {
  }
  return listen<boolean>('native-window-active', (e) => {
    setNativeWindowActive(e.payload);
    onChange?.(e.payload);
  });
}

export type NativeWindowShownTracking = {
  status: 'supported' | 'unsupported';
  stop(): void;
};

export async function startNativeWindowShownTracking(
  onChange: (shown: boolean) => void,
  signal?: AbortSignal,
): Promise<NativeWindowShownTracking> {
  if (!isTauri()) return { status: 'unsupported', stop() {} };
  let transitioned = false;
  let stopped = false;
  const unlisten = await listen<boolean>('native-window-shown', (event) => {
    if (stopped || signal?.aborted) return;
    transitioned = true;
    onChange(event.payload);
  });
  const stop = () => {
    if (stopped) return;
    stopped = true;
    signal?.removeEventListener('abort', stop);
    unlisten();
  };
  signal?.addEventListener('abort', stop, { once: true });
  try {
    if (signal?.aborted) throw new Error('Native window visibility tracking stopped.');
    const shown = await getNativeWindowShown();
    if (signal?.aborted) throw new Error('Native window visibility tracking stopped.');
    if (!transitioned) onChange(shown);
    return { status: 'supported', stop };
  } catch (error) {
    stop();
    const message = error instanceof Error ? error.message : error;
    if (message === 'Command get_native_window_shown not allowed by ACL'
      || message === 'Command get_native_window_shown not found') {
      return { status: 'unsupported', stop };
    }
    throw error;
  }
}
