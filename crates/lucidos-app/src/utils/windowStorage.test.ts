import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  MAX_REMEMBERED_WINDOWS,
  POSSIBLY_OPEN_MS,
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

  it('keeps only the most recently seen windows when many closed within the TTL', () => {
    const windows: string[] = [];
    for (let i = 0; i < MAX_REMEMBERED_WINDOWS * 3; i++) {
      becomeAnotherWindow();
      writeWindowItem(KEY, `thread-${i}`);
      windows.push(windowId());
    }
    const lastSeen = JSON.parse(localStorage.getItem(WINDOW_LAST_SEEN_KEY)!);
    windows.forEach((id, i) => { lastSeen[id].seen = 1_000 + i; });
    localStorage.setItem(WINDOW_LAST_SEEN_KEY, JSON.stringify(lastSeen));
    const self = windows[0];
    sessionStorage.setItem(TAB_WINDOW_ID_KEY, self);
    _resetWindowStorageForTests();

    pruneClosedWindows(POSSIBLY_OPEN_MS + 2_000);

    const kept = [self, ...windows.slice(-(MAX_REMEMBERED_WINDOWS - 1))];
    expect(Object.keys(JSON.parse(localStorage.getItem(WINDOW_LAST_SEEN_KEY)!)).sort()).toEqual([...kept].sort());
    for (const id of windows) {
      const expected = kept.includes(id) ? `thread-${windows.indexOf(id)}` : null;
      expect(localStorage.getItem(`${KEY}@window:${id}`)).toBe(expected);
    }
  });

  it('never prunes a window seen recently enough to still be open, even past the cap', () => {
    writeWindowItem(KEY, 'thread-open');
    const open = windowId();
    becomeAnotherWindow();
    writeWindowItem(KEY, 'thread-self');
    const now = Date.now();

    expect(pruneClosedWindows(now, 1)).toBe(0);
    expect(localStorage.getItem(`${KEY}@window:${open}`)).toBe('thread-open');

    expect(pruneClosedWindows(now + POSSIBLY_OPEN_MS, 1)).toBe(1);
    expect(localStorage.getItem(`${KEY}@window:${open}`)).toBeNull();
  });

  it('never prunes a window a peer reports open, however long it sat idle', () => {
    writeWindowItem(KEY, 'thread-idle');
    const idle = windowId();
    becomeAnotherWindow();
    writeWindowItem(KEY, 'thread-self');

    expect(pruneClosedWindows(Date.now() + WINDOW_STATE_TTL_MS, 1, new Set([idle]))).toBe(0);
    expect(localStorage.getItem(`${KEY}@window:${idle}`)).toBe('thread-idle');
  });

  it('keeps the keys of a window a peer reports open even when the registry lost it', () => {
    writeWindowItem(KEY, 'thread-self');
    localStorage.setItem(`${KEY}@window:open-peer`, 'peer state');

    expect(pruneClosedWindows(Date.now(), 1, new Set(['open-peer']))).toBe(0);
    expect(localStorage.getItem(`${KEY}@window:open-peer`)).toBe('peer state');
  });

  it('frees orphaned window keys even when storage refuses the registry write', () => {
    writeWindowItem(KEY, 'thread-self');
    localStorage.setItem(`${KEY}@window:untracked`, 'orphan');
    const original = localStorage.setItem;
    localStorage.setItem = (key: string, value: string) => {
      if (key === WINDOW_LAST_SEEN_KEY) throw new DOMException('full', 'QuotaExceededError');
      original.call(localStorage, key, value);
    };
    try {
      expect(pruneClosedWindows()).toBe(1);
    } finally {
      localStorage.setItem = original;
    }
    expect(localStorage.getItem(`${KEY}@window:untracked`)).toBeNull();
    expect(localStorage.getItem(`${KEY}@window:${windowId()}`)).toBe('thread-self');
  });

  it('sweeps window keys whose window the registry does not hold', () => {
    writeWindowItem(KEY, 'thread-self');
    localStorage.setItem(`${KEY}@window:untracked`, 'orphan');
    localStorage.setItem(`lucidos-nav-history@window:${windowId()}`, 'unlisted but mine');

    expect(pruneClosedWindows()).toBe(1);

    expect(localStorage.getItem(`${KEY}@window:untracked`)).toBeNull();
    expect(localStorage.getItem(`${KEY}@window:${windowId()}`)).toBe('thread-self');
    expect(localStorage.getItem(`lucidos-nav-history@window:${windowId()}`)).toBe('unlisted but mine');
  });

  it('leaves an app key that happens to contain the window marker', () => {
    const appKey = 'appbridge:habit-tracker:app:layout@window:main';
    localStorage.setItem(appKey, 'app data');

    pruneClosedWindows();

    expect(localStorage.getItem(appKey)).toBe('app data');
  });

  it('removes every key of a pruned window, even one its registry entry never listed', () => {
    writeWindowItem(KEY, 'thread-closed');
    const closed = windowId();
    localStorage.setItem(`lucidos-nav-history@window:${closed}`, 'unlisted');
    becomeAnotherWindow();
    writeWindowItem(KEY, 'thread-self');

    expect(pruneClosedWindows(Date.now() + POSSIBLY_OPEN_MS, 1)).toBe(1);

    expect(localStorage.getItem(`${KEY}@window:${closed}`)).toBeNull();
    expect(localStorage.getItem(`lucidos-nav-history@window:${closed}`)).toBeNull();
  });

  it('records a window before its key, so a refused write leaves nothing untracked', () => {
    const original = localStorage.setItem;
    localStorage.setItem = (key: string, value: string) => {
      if (key === WINDOW_LAST_SEEN_KEY) throw new DOMException('full', 'QuotaExceededError');
      original.call(localStorage, key, value);
    };
    try {
      writeWindowItem(KEY, 'thread-a');
    } finally {
      localStorage.setItem = original;
    }
    expect(localStorage.getItem(`${KEY}@window:${windowId()}`)).toBeNull();
  });

  it('tracks a key whose first record was refused once the retry succeeds', () => {
    writeWindowItem('lucidos-other', 'stamped');
    const original = localStorage.setItem;
    localStorage.setItem = (key: string, value: string) => {
      if (key === WINDOW_LAST_SEEN_KEY) throw new DOMException('full', 'QuotaExceededError');
      original.call(localStorage, key, value);
    };
    try {
      writeWindowItem(KEY, 'refused');
    } finally {
      localStorage.setItem = original;
    }

    writeWindowItem(KEY, 'thread-a');

    expect(localStorage.getItem(`${KEY}@window:${windowId()}`)).toBe('thread-a');
    expect(JSON.parse(localStorage.getItem(WINDOW_LAST_SEEN_KEY)!)[windowId()].keys).toContain(KEY);
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
