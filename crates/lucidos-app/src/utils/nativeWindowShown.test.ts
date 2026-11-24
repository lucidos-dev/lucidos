import { beforeEach, expect, it, vi } from 'vitest';

const bridge = vi.hoisted(() => ({
  getNativeWindowShown: vi.fn(),
  listen: vi.fn(),
  stop: vi.fn(),
  handler: (_event: { payload: boolean }) => {},
}));
vi.mock('./platform', () => ({ isTauri: () => true }));
vi.mock('./tauri', () => ({
  getNativeWindowActive: vi.fn(),
  getNativeWindowShown: bridge.getNativeWindowShown,
  listen: bridge.listen,
}));
import { startNativeWindowShownTracking } from './nativeWindow';

beforeEach(() => {
  bridge.stop.mockReset();
  bridge.getNativeWindowShown.mockReset().mockResolvedValue(false);
  bridge.listen.mockReset().mockImplementation(async (_name, handler) => {
    bridge.handler = handler;
    return bridge.stop;
  });
});

it('seeds a hidden window, follows show/hide independently of focus, and tears down', async () => {
  const changed = vi.fn();
  const { stop } = await startNativeWindowShownTracking(changed);
  expect(changed).toHaveBeenLastCalledWith(false);
  expect(bridge.listen).toHaveBeenCalledWith('native-window-shown', expect.any(Function));
  bridge.handler({ payload: true });
  expect(changed).toHaveBeenLastCalledWith(true);
  bridge.handler({ payload: false });
  expect(changed).toHaveBeenLastCalledWith(false);
  stop();
  expect(bridge.stop).toHaveBeenCalled();
});

it('a transition during the seed wins over the stale command response', async () => {
  bridge.getNativeWindowShown.mockImplementation(async () => {
    bridge.handler({ payload: true });
    return false;
  });
  const changed = vi.fn();
  await startNativeWindowShownTracking(changed);
  expect(changed.mock.calls).toEqual([[true]]);
});

it.each([
  'Command get_native_window_shown not allowed by ACL',
  'Command get_native_window_shown not found',
])('recognizes legacy visibility support: %s', async (error) => {
  bridge.getNativeWindowShown.mockRejectedValueOnce(error);
  const changed = vi.fn();
  const result = await startNativeWindowShownTracking(changed);
  expect(result.status).toBe('unsupported');
  expect(changed).not.toHaveBeenCalled();
  expect(bridge.stop).toHaveBeenCalledTimes(1);
});

it.each([
  'IPC connection closed',
  'Command get_native_window_active not allowed by ACL',
  'unexpected get_native_window_shown failure',
])('propagates genuine seed failure and releases its listener: %s', async (error) => {
  bridge.getNativeWindowShown.mockRejectedValueOnce(error);
  await expect(startNativeWindowShownTracking(vi.fn())).rejects.toBe(error);
  expect(bridge.stop).toHaveBeenCalledTimes(1);
});

it('propagates listener failure without probing compatibility', async () => {
  bridge.listen.mockRejectedValueOnce(new Error('event subscription failed'));
  await expect(startNativeWindowShownTracking(vi.fn())).rejects.toThrow('event subscription failed');
  expect(bridge.getNativeWindowShown).not.toHaveBeenCalled();
});

it('releases a listener immediately on teardown while its seed is pending', async () => {
  let seed!: (shown: boolean) => void;
  bridge.getNativeWindowShown.mockImplementationOnce(() => new Promise(resolve => { seed = resolve; }));
  const lifetime = new AbortController();
  const changed = vi.fn();
  const tracking = startNativeWindowShownTracking(changed, lifetime.signal);
  await Promise.resolve();
  lifetime.abort();
  expect(bridge.stop).toHaveBeenCalledTimes(1);
  bridge.handler({ payload: true });
  seed(false);
  await expect(tracking).rejects.toThrow('tracking stopped');
  expect(changed).not.toHaveBeenCalled();
  expect(bridge.stop).toHaveBeenCalledTimes(1);
});
