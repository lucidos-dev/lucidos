import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  WINDOW_LAST_SEEN_KEY,
  WINDOW_STATE_TTL_MS,
  _resetWindowStorageForTests,
  pruneClosedWindows,
  readWindowItem,
  reassignWindowId,
  removeWindowItem,
  touchWindowSeen,
  windowId,
  writeWindowItem,
} from './windowStorage';
import { TAB_WINDOW_ID_KEY } from './windowIdKey';

const KEY = 'lucidos-focused-thread';

/** Boot as another window: a fresh tab id, as a second window would have. */
function becomeAnotherWindow(): void {
  sessionStorage.clear();
  _resetWindowStorageForTests();
}

describe('window-scoped storage', () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    _resetWindowStorageForTests();
  });
  afterEach(() => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    _resetWindowStorageForTests();
  });

  it('takes the Tauri label as the window id on the desktop', () => {
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'window-abc' } },
    };
    expect(windowId()).toBe('window-abc');
  });

  it('keeps a browser tab id across a reload', () => {
    const first = windowId();
    _resetWindowStorageForTests();
    expect(windowId()).toBe(first);
  });

  it("restores this window's own value over a newer write by another window", () => {
    writeWindowItem(KEY, 'thread-a');
    const a = sessionStorage.getItem(TAB_WINDOW_ID_KEY);
    becomeAnotherWindow();
    writeWindowItem(KEY, 'thread-b');
    expect(readWindowItem(KEY)).toBe('thread-b');

    sessionStorage.setItem(TAB_WINDOW_ID_KEY, a!);
    _resetWindowStorageForTests();
    expect(readWindowItem(KEY)).toBe('thread-a');
  });

  it('starts a window with no state of its own from the last navigation', () => {
    writeWindowItem(KEY, 'thread-a');
    becomeAnotherWindow();
    expect(readWindowItem(KEY)).toBe('thread-a');
  });

  it("keeps a removal, rather than falling through to another window's seed", () => {
    writeWindowItem(KEY, 'thread-a');
    const a = sessionStorage.getItem(TAB_WINDOW_ID_KEY);
    removeWindowItem(KEY);
    becomeAnotherWindow();
    writeWindowItem(KEY, 'thread-b');

    sessionStorage.setItem(TAB_WINDOW_ID_KEY, a!);
    _resetWindowStorageForTests();
    expect(readWindowItem(KEY)).toBeNull();
  });

  it('removes the keys of a window gone longer than the TTL, and keeps the rest', () => {
    writeWindowItem(KEY, 'thread-a');
    const gone = windowId();
    becomeAnotherWindow();
    writeWindowItem(KEY, 'thread-b');
    const live = windowId();
    const lastSeen = JSON.parse(localStorage.getItem(WINDOW_LAST_SEEN_KEY)!);
    lastSeen[gone].seen = 0;
    lastSeen[live].seen = WINDOW_STATE_TTL_MS;
    localStorage.setItem(WINDOW_LAST_SEEN_KEY, JSON.stringify(lastSeen));

    pruneClosedWindows(WINDOW_STATE_TTL_MS + 1);

    expect(localStorage.getItem(`${KEY}@window:${gone}`)).toBeNull();
    expect(localStorage.getItem(`${KEY}@window:${live}`)).toBe('thread-b');
    expect(Object.keys(JSON.parse(localStorage.getItem(WINDOW_LAST_SEEN_KEY)!))).toEqual([live]);
  });

  it('keeps a window in use from being pruned, however long it stays open', () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(0);
      writeWindowItem(KEY, 'thread-a');
      const longLived = windowId();
      vi.setSystemTime(WINDOW_STATE_TTL_MS);
      touchWindowSeen();
      becomeAnotherWindow();
      pruneClosedWindows(WINDOW_STATE_TTL_MS + 1);
      expect(localStorage.getItem(`${KEY}@window:${longLived}`)).toBe('thread-a');
    } finally {
      vi.useRealTimers();
    }
  });

  it('gives a duplicated tab a new id that keeps what it showed', () => {
    writeWindowItem(KEY, 'thread-a');
    const original = windowId();
    expect(reassignWindowId()).toBe(true);
    expect(windowId()).not.toBe(original);
    expect(sessionStorage.getItem(TAB_WINDOW_ID_KEY)).toBe(windowId());
    expect(readWindowItem(KEY)).toBe('thread-a');
    writeWindowItem(KEY, 'thread-b');
    expect(localStorage.getItem(`${KEY}@window:${original}`)).toBe('thread-a');
  });

  it('never reassigns a desktop label', () => {
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
    };
    expect(reassignWindowId()).toBe(false);
    expect(windowId()).toBe('main');
  });

  it('never throws when storage refuses a write', () => {
    const original = localStorage.setItem;
    localStorage.setItem = () => { throw new Error('quota'); };
    try {
      expect(() => writeWindowItem(KEY, 'thread-a')).not.toThrow();
      expect(() => removeWindowItem(KEY)).not.toThrow();
      expect(() => pruneClosedWindows()).not.toThrow();
    } finally {
      localStorage.setItem = original;
    }
  });
});
