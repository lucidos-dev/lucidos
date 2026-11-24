import { afterEach, expect, it, vi } from 'vitest';

const bridge = vi.hoisted(() => ({
  seed: vi.fn(async () => false),
  handler: (_event: { payload: boolean }) => {},
  stop: vi.fn(),
}));
vi.mock('../utils/platform', () => ({ isTauri: () => true }));
vi.mock('../utils/tauri', () => ({
  currentWindowLabel: () => 'self',
  getNativeWindowShown: bridge.seed,
  getNativeWindowActive: vi.fn(),
  listen: vi.fn(async (_event, handler) => {
    bridge.handler = handler;
    return bridge.stop;
  }),
}));
import { WORKSPACE_ID } from '../utils/basePath';
import { installWindowPeers, isChosenForThread, isChosenByPreference, knownWindows, waitForNativeWindowShown } from './windowPeers';
import { windowId } from '../utils/windowStorage';

let stop = () => {};
const channels: BroadcastChannel[] = [];
afterEach(() => {
  stop();
  channels.splice(0).forEach((channel) => channel.close());
  vi.clearAllMocks();
});
const settle = () => new Promise((resolve) => setTimeout(resolve, 30));
async function peerShown(shown: boolean, thread = 'source') {
  const other = new BroadcastChannel(`lucidos-window-peers:${WORKSPACE_ID ?? ''}`);
  channels.push(other);
  other.postMessage({ type: 'state', instance: 'peer', state: {
    id: 'other', focusedThreadId: thread, appId: 'target', authorizing: null,
    activeAt: Date.now() + 1000, nativeShown: shown,
  } });
  await settle();
}

it('announces the native hidden seed and show/hide transitions through the real subscription', async () => {
  const observer = new BroadcastChannel(`lucidos-window-peers:${WORKSPACE_ID ?? ''}`);
  channels.push(observer);
  const announcements: unknown[] = [];
  observer.onmessage = (event) => announcements.push(event.data.state?.nativeShown);
  stop = installWindowPeers(() => ({ focusedThreadId: 'source', appId: null, authorizing: null }));
  await settle();
  expect(announcements).toContain(false);
  await peerShown(true, 'elsewhere');
  expect(isChosenForThread('source')).toBe(false);
  bridge.handler({ payload: true });
  await settle();
  expect(announcements).toContain(true);
  expect(isChosenForThread('source')).toBe(true);
  bridge.handler({ payload: false });
  await settle();
  expect(isChosenForThread('source')).toBe(false);
  expect(knownWindows().find((window) => window.id === windowId())?.nativeShown).toBe(false);
  stop();
  expect(bridge.stop).toHaveBeenCalledTimes(1);
  stop = () => {};
});

it('shown peers beat hidden source owners, including previous-view grace; captures keep app preference', async () => {
  stop = installWindowPeers(() => ({ focusedThreadId: 'elsewhere', appId: null, authorizing: null }));
  await settle();
  bridge.handler({ payload: true });
  await peerShown(true);
  expect(isChosenForThread('source')).toBe(false);
  await peerShown(false, 'moved');
  expect(isChosenForThread('source')).toBe(true);
  expect(isChosenByPreference((window) => window.appId === 'target', () => true)).toBe(false);
  await peerShown(true);
  expect(isChosenForThread('source')).toBe(false);
});

it('all hidden preserves the existing thread preference and lifetime', async () => {
  stop = installWindowPeers(() => ({ focusedThreadId: 'source', appId: null, authorizing: null }));
  await settle();
  await peerShown(false, 'elsewhere');
  expect(isChosenForThread('source')).toBe(true);
  expect(knownWindows()).toHaveLength(2);
});

it('releases a late subscription when peers were already torn down', async () => {
  let resolveSeed!: (shown: boolean) => void;
  bridge.seed.mockImplementationOnce(() => new Promise((resolve) => { resolveSeed = resolve; }));
  stop = installWindowPeers(() => ({ focusedThreadId: null, appId: null, authorizing: null }));
  await Promise.resolve();
  stop();
  stop = () => {};
  resolveSeed(false);
  await settle();
  expect(bridge.stop).toHaveBeenCalledTimes(1);
});

it('holds delivery readiness until the native seed settles', async () => {
  let seed!: (shown: boolean) => void;
  bridge.seed.mockImplementationOnce(() => new Promise((resolve) => { seed = resolve; }));
  stop = installWindowPeers(() => ({ focusedThreadId: null, appId: null, authorizing: null }));
  let ready = false;
  const waiting = waitForNativeWindowShown().then(() => { ready = true; });
  await settle();
  expect(ready).toBe(false);
  seed(false);
  await waiting;
  expect(ready).toBe(true);
});

it('retries failed initialization once for concurrent callers and cleans up both attempts', async () => {
  bridge.seed.mockRejectedValueOnce(new Error('IPC connection closed'));
  stop = installWindowPeers(() => ({ focusedThreadId: null, appId: null, authorizing: null }));
  await expect(waitForNativeWindowShown()).rejects.toThrow('IPC connection closed');
  expect(bridge.stop).toHaveBeenCalledTimes(1);
  await Promise.all([waitForNativeWindowShown(), waitForNativeWindowShown()]);
  expect(bridge.seed).toHaveBeenCalledTimes(2);
  expect(knownWindows().find((window) => window.id === windowId())?.nativeShown).toBe(false);
  stop();
  stop = () => {};
  expect(bridge.stop).toHaveBeenCalledTimes(2);
});

it('discards transitions when the seed establishes that visibility is unsupported', async () => {
  bridge.seed.mockImplementationOnce(async () => {
    bridge.handler({ payload: false });
    throw new Error('Command get_native_window_shown not allowed by ACL');
  });
  stop = installWindowPeers(() => ({ focusedThreadId: 'source', appId: null, authorizing: null }));
  await waitForNativeWindowShown();
  expect(knownWindows().find((window) => window.id === windowId())?.nativeShown).toBeUndefined();
  bridge.handler({ payload: false });
  expect(knownWindows().find((window) => window.id === windowId())?.nativeShown).toBeUndefined();
});
