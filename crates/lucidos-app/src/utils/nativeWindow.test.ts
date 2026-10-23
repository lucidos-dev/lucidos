import { describe, it, expect, beforeEach, vi } from 'vitest';

const isTauri = vi.fn(() => true);
vi.mock('./platform', () => ({ isTauri: () => isTauri() }));

// The Rust `get_native_window_active` seed command + the transition listener.
const getNativeWindowActive = vi.fn((): Promise<boolean> => Promise.resolve(false));
let listenHandler: ((e: { payload: boolean }) => void) | null = null;
const listenUnlisten = vi.fn();
const listen = vi.fn((_event: string, handler: (e: { payload: boolean }) => void) => {
  listenHandler = handler;
  return Promise.resolve(listenUnlisten);
});
vi.mock('./tauri', () => ({
  getNativeWindowActive: () => getNativeWindowActive(),
  listen: (event: string, handler: (e: { payload: boolean }) => void) => listen(event, handler),
}));

import {
  startNativeWindowActiveTracking,
  isNativeWindowActive,
  setNativeWindowActive,
} from './nativeWindow';

describe('startNativeWindowActiveTracking — seed before listen', () => {
  beforeEach(() => {
    isTauri.mockReturnValue(true);
    setNativeWindowActive(true); // reset cache to its `true` default
    getNativeWindowActive.mockReset();
    getNativeWindowActive.mockResolvedValue(false);
    listen.mockClear();
    listenHandler = null;
  });

  it('seeds the cache from the command, correcting the true default', async () => {
    // The "only sometimes" fix: a backgrounded/trayed window reports inactive;
    // without the seed the cache stays at its `true` default and the device
    // wrongly pongs active, so the engine suppresses the banner.
    const changes: boolean[] = [];
    await startNativeWindowActiveTracking((a) => changes.push(a));
    expect(getNativeWindowActive).toHaveBeenCalledTimes(1);
    expect(isNativeWindowActive()).toBe(false);
    expect(changes).toContain(false); // onChange fired with the seeded value
    expect(listen).toHaveBeenCalledWith('native-window-active', expect.any(Function));
  });

  it('keeps tracking transitions after the seed', async () => {
    await startNativeWindowActiveTracking();
    expect(listenHandler).toBeTruthy();
    listenHandler!({ payload: true });
    expect(isNativeWindowActive()).toBe(true);
    listenHandler!({ payload: false });
    expect(isNativeWindowActive()).toBe(false);
  });

  it('a failed seed leaves the cache at its default and still listens', async () => {
    getNativeWindowActive.mockRejectedValueOnce(new Error('ipc down'));
    setNativeWindowActive(true);
    await startNativeWindowActiveTracking();
    expect(isNativeWindowActive()).toBe(true); // unchanged default (best-effort)
    expect(listen).toHaveBeenCalled();
  });

  it('is a no-op off Tauri (browser / PWA)', async () => {
    isTauri.mockReturnValue(false);
    const un = await startNativeWindowActiveTracking();
    expect(getNativeWindowActive).not.toHaveBeenCalled();
    expect(listen).not.toHaveBeenCalled();
    expect(typeof un).toBe('function');
  });
});
