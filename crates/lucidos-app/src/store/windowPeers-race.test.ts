import { afterEach, expect, it, vi } from 'vitest';

class QueuedChannel {
  static channels = new Set<QueuedChannel>();
  static messages: Array<() => void> = [];
  onmessage: ((event: { data: unknown }) => void) | null = null;
  constructor(readonly name: string) { QueuedChannel.channels.add(this); }
  postMessage(data: unknown) {
    for (const peer of QueuedChannel.channels) {
      if (peer !== this && peer.name === this.name) {
        QueuedChannel.messages.push(() => peer.onmessage?.({ data }));
      }
    }
  }
  close() { QueuedChannel.channels.delete(this); }
  static flush() {
    while (this.messages.length) this.messages.shift()!();
  }
}

const stops: Array<() => void> = [];
afterEach(() => {
  stops.splice(0).forEach((stop) => stop());
  QueuedChannel.channels.clear();
  QueuedChannel.messages = [];
  vi.unstubAllGlobals();
  vi.doUnmock('../utils/nativeWindow');
  vi.doUnmock('../utils/windowStorage');
  vi.resetModules();
});

async function runtime(id: string, initialShown: boolean, thread: string) {
  vi.resetModules();
  let shownChanged = (_shown: boolean) => {};
  vi.doMock('../utils/nativeWindow', () => ({
    startNativeWindowShownTracking: async (changed: (shown: boolean) => void) => {
      shownChanged = changed;
      changed(initialShown);
      return { status: 'supported', stop() {} };
    },
  }));
  vi.doMock('../utils/windowStorage', () => ({
    windowId: () => id, touchWindowSeen: () => {}, reassignWindowId: () => false,
  }));
  const peers = await import('./windowPeers');
  stops.push(peers.installWindowPeers(() => ({
    focusedThreadId: thread, appId: null, authorizing: null,
  })));
  return { ...peers, setShown: (shown: boolean) => shownChanged(shown) };
}

async function pair() {
  vi.stubGlobal('BroadcastChannel', QueuedChannel);
  const owner = await runtime('owner', true, 'source');
  const other = await runtime('other', true, 'elsewhere');
  QueuedChannel.flush();
  expect(owner.knownWindows()).toHaveLength(2);
  expect(other.knownWindows()).toHaveLength(2);
  return { owner, other };
}

it('reproduces a lost request when hide arrives locally before its announcement', async () => {
  const { owner, other } = await pair();
  owner.setShown(false);
  const accepted = [owner, other].filter((page) => page.isChosenForThread('source'));
  expect(accepted).toHaveLength(0);
  QueuedChannel.flush();
  expect(other.isChosenForThread('source')).toBe(true);
});

it('reproduces duplicate presentation when show precedes its announcement', async () => {
  const { owner, other } = await pair();
  owner.setShown(false);
  QueuedChannel.flush();
  owner.setShown(true);
  const accepted = [owner, other].filter((page) => page.isChosenForThread('source'));
  expect(accepted).toHaveLength(2);
  QueuedChannel.flush();
  expect(other.isChosenForThread('source')).toBe(false);
});
