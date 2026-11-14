import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  chooseWindow,
  installWindowPeers,
  isChosenByPreference,
  isChosenWindow,
  PEER_CHANGE_GRACE_MS,
  whenWindowSeen,
  type PeerState,
} from './windowPeers';
import { WORKSPACE_ID } from '../utils/basePath';
import { _resetWindowStorageForTests, windowId } from '../utils/windowStorage';

function peer(id: string, overrides: Partial<PeerState> = {}): PeerState {
  return { id, focusedThreadId: null, appId: null, authorizing: null, activeAt: 0, ...overrides };
}

describe('chooseWindow', () => {
  it('picks the last-touched window among those that qualify', () => {
    const windows = [
      peer('a', { focusedThreadId: 't1', activeAt: 10 }),
      peer('b', { focusedThreadId: 't1', activeAt: 20 }),
      peer('c', { focusedThreadId: 't2', activeAt: 30 }),
    ];
    expect(chooseWindow(windows, (w) => w.focusedThreadId === 't1')).toBe('b');
  });

  it('breaks a tie by id, so every window picks the same', () => {
    const windows = [peer('b', { activeAt: 5 }), peer('a', { activeAt: 5 })];
    expect(chooseWindow(windows, () => true)).toBe('a');
    expect(chooseWindow([...windows].reverse(), () => true)).toBe('a');
  });

  it('is null when no window qualifies', () => {
    expect(chooseWindow([peer('a')], () => false)).toBeNull();
  });
});

describe('the windows of one workspace', () => {
  let other: BroadcastChannel;
  let stop: () => void;
  let self: Omit<PeerState, 'id' | 'activeAt'>;

  /** Another window, speaking on the same channel. */
  function otherSays(message: unknown): Promise<void> {
    other.postMessage(message);
    return new Promise((resolve) => setTimeout(resolve, 20));
  }

  beforeEach(() => {
    sessionStorage.clear();
    _resetWindowStorageForTests();
    self = { focusedThreadId: 't1', appId: null, authorizing: null };
    other = new BroadcastChannel(`lucidos-window-peers:${WORKSPACE_ID ?? ''}`);
    stop = installWindowPeers(() => self);
  });
  afterEach(() => {
    stop();
    other.close();
    vi.restoreAllMocks();
  });

  it('acts alone when no other window is known', () => {
    expect(isChosenWindow((w) => w.focusedThreadId === 't1')).toBe(true);
  });

  it('defers to a window touched more recently that shows the same thread', async () => {
    await otherSays({ type: 'state', state: peer('other', { focusedThreadId: 't1', activeAt: Date.now() + 1000 }) });
    expect(isChosenWindow((w) => w.focusedThreadId === 't1')).toBe(false);
  });

  it('acts when the other window shows something else', async () => {
    await otherSays({ type: 'state', state: peer('other', { focusedThreadId: 't2', activeAt: Date.now() + 1000 }) });
    expect(isChosenByPreference((w) => w.focusedThreadId === 't1', () => true)).toBe(true);
  });

  it('leaves the fallback to the last-touched window when none qualifies first', async () => {
    self = { focusedThreadId: 't3', appId: null, authorizing: null };
    await otherSays({ type: 'state', state: peer('other', { focusedThreadId: 't2', activeAt: Date.now() + 1000 }) });
    expect(isChosenByPreference((w) => w.focusedThreadId === 't1', () => true)).toBe(false);
  });

  it('forgets a window that said goodbye', async () => {
    await otherSays({ type: 'state', state: peer('other', { focusedThreadId: 't1', activeAt: Date.now() + 1000 }) });
    await otherSays({ type: 'bye', id: 'other' });
    expect(isChosenWindow((w) => w.focusedThreadId === 't1')).toBe(true);
  });

  it("answers a newcomer's hello with what it shows", async () => {
    const heard: unknown[] = [];
    other.onmessage = (event) => heard.push(event.data);
    await otherSays({ type: 'hello', state: peer('newcomer') });
    expect(heard).toContainEqual(expect.objectContaining({
      type: 'state',
      state: expect.objectContaining({ id: windowId(), focusedThreadId: 't1' }),
    }));
  });

  it('ignores a goodbye from an instance that has since reloaded', async () => {
    await otherSays({ type: 'state', instance: 'new', state: peer('other', { focusedThreadId: 't1', activeAt: Date.now() + 1000 }) });
    await otherSays({ type: 'bye', id: 'other', instance: 'old' });
    expect(isChosenWindow((w) => w.focusedThreadId === 't1')).toBe(false);
  });

  it('still counts a window for what it showed just before it changed', async () => {
    await otherSays({ type: 'state', instance: 'x', state: peer('other', { authorizing: { provider: null }, activeAt: 1 }) });
    await otherSays({ type: 'state', instance: 'x', state: peer('other', { authorizing: null, activeAt: 1 }) });
    expect(isChosenByPreference((w) => w.authorizing !== null, () => true)).toBe(false);
  });

  it('lets that grace run out', async () => {
    await otherSays({ type: 'state', instance: 'x', state: peer('other', { authorizing: { provider: null }, activeAt: 1 }) });
    await otherSays({ type: 'state', instance: 'x', state: peer('other', { authorizing: null, activeAt: 1 }) });
    const now = Date.now();
    vi.spyOn(Date, 'now').mockReturnValue(now + PEER_CHANGE_GRACE_MS);
    expect(isChosenByPreference((w) => w.authorizing !== null, () => true)).toBe(true);
  });

  it('forgets a window whose instance died without a goodbye', async () => {
    if (!navigator.locks) return;
    await otherSays({ type: 'state', instance: 'crashed', alive: true, state: peer('other', { focusedThreadId: 't1', activeAt: Date.now() + 1000 }) });
    expect(isChosenWindow((w) => w.focusedThreadId === 't1')).toBe(true);
  });

  it('keeps a window whose instance still holds its life lock', async () => {
    if (!navigator.locks) return;
    let release = () => {};
    void navigator.locks.request('lucidos-window-alive:living', () => new Promise<void>((resolve) => { release = resolve; }));
    await otherSays({ type: 'state', instance: 'living', alive: true, state: peer('other', { focusedThreadId: 't1', activeAt: Date.now() + 1000 }) });
    expect(isChosenWindow((w) => w.focusedThreadId === 't1')).toBe(false);
    release();
  });

  it('takes a new id when an older tab already holds this one', async () => {
    const before = windowId();
    await otherSays({ type: 'state', instance: 'elder', bootedAt: 0, state: peer(before) });
    expect(windowId()).not.toBe(before);
  });

  it('keeps its id against a younger tab, and tells it so', async () => {
    const heard: unknown[] = [];
    other.onmessage = (event) => heard.push(event.data);
    const before = windowId();
    await otherSays({ type: 'hello', instance: 'younger', bootedAt: Date.now() + 60_000, state: peer(before) });
    expect(windowId()).toBe(before);
    expect(heard).toContainEqual(expect.objectContaining({ type: 'state' }));
  });

  it('tells a listener once a named window is heard from', async () => {
    const seen = vi.fn();
    whenWindowSeen('other', seen);
    await otherSays({ type: 'state', state: peer('someone-else') });
    expect(seen).not.toHaveBeenCalled();
    await otherSays({ type: 'state', state: peer('other') });
    expect(seen).toHaveBeenCalledTimes(1);
  });
});
