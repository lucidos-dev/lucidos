import { describe, it, expect } from 'vitest';
import type { PanelOverlay } from '../store';

// NavEntry and pure functions are tested directly without importing the
// navigation module (which pulls in store.ts → localStorage at module level).
// We duplicate the pure logic here to test it in isolation.

interface NavEntry {
  menuItem: string;
  settingsSubview: string;
  overlay: PanelOverlay;
}

function overlaysEqual(a: PanelOverlay, b: PanelOverlay): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  if (a.type !== b.type) return false;
  switch (a.type) {
    case 'form': return JSON.stringify(a.form) === JSON.stringify((b as typeof a).form);
    case 'app-ui': return a.app.id === (b as typeof a).app.id;
    case 'file-preview': return a.path === (b as typeof a).path;
    case 'url-preview': return a.url === (b as typeof a).url;
    case 'notification-detail': return a.notification.id === (b as typeof a).notification.id;
  }
}

function statesEqual(a: NavEntry, b: NavEntry): boolean {
  return (
    a.menuItem === b.menuItem &&
    a.settingsSubview === b.settingsSubview &&
    overlaysEqual(a.overlay, b.overlay)
  );
}

const MAX_NAV_STACK = 50;

function pushEntry(
  stack: NavEntry[],
  cursor: number,
  entry: NavEntry,
): { stack: NavEntry[]; cursor: number } | null {
  if (cursor < stack.length) {
    if (entry.overlay && overlaysEqual(entry.overlay, stack[cursor].overlay)) return null;
    if (statesEqual(entry, stack[cursor])) return null;
  }
  let newStack = [...stack.slice(0, cursor + 1), entry];
  let newCursor = newStack.length - 1;
  if (newStack.length > MAX_NAV_STACK) {
    const overflow = newStack.length - MAX_NAV_STACK;
    newStack = newStack.slice(overflow);
    newCursor -= overflow;
  }
  return { stack: newStack, cursor: newCursor };
}

function makeEntry(overrides: Partial<NavEntry> & { overlay?: PanelOverlay } = {}): NavEntry {
  return {
    menuItem: 'files',
    settingsSubview: 'main',
    overlay: null,
    ...overrides,
  };
}

describe('statesEqual', () => {
  it('identical entries are equal', () => {
    expect(statesEqual(makeEntry(), makeEntry())).toBe(true);
  });

  it('different menuItem', () => {
    expect(statesEqual(makeEntry(), makeEntry({ menuItem: 'settings' }))).toBe(false);
  });

  it('different settingsSubview', () => {
    expect(statesEqual(makeEntry(), makeEntry({ settingsSubview: 'accounts' }))).toBe(false);
  });

  it('null vs file-preview overlay', () => {
    expect(statesEqual(
      makeEntry(),
      makeEntry({ overlay: { type: 'file-preview', path: 'notes.md' } }),
    )).toBe(false);
  });

  it('different file-preview paths', () => {
    expect(statesEqual(
      makeEntry({ overlay: { type: 'file-preview', path: 'a.md' } }),
      makeEntry({ overlay: { type: 'file-preview', path: 'b.md' } }),
    )).toBe(false);
  });

  it('same file-preview paths', () => {
    expect(statesEqual(
      makeEntry({ overlay: { type: 'file-preview', path: 'a.md' } }),
      makeEntry({ overlay: { type: 'file-preview', path: 'a.md' } }),
    )).toBe(true);
  });

  it('different url-preview urls', () => {
    expect(statesEqual(
      makeEntry({ overlay: { type: 'url-preview', url: 'https://a.com' } }),
      makeEntry({ overlay: { type: 'url-preview', url: 'https://b.com' } }),
    )).toBe(false);
  });

  it('same url-preview urls', () => {
    expect(statesEqual(
      makeEntry({ overlay: { type: 'url-preview', url: 'https://a.com' } }),
      makeEntry({ overlay: { type: 'url-preview', url: 'https://a.com' } }),
    )).toBe(true);
  });

  it('different overlay types', () => {
    expect(statesEqual(
      makeEntry({ overlay: { type: 'file-preview', path: 'a.md' } }),
      makeEntry({ overlay: { type: 'url-preview', url: 'https://a.com' } }),
    )).toBe(false);
  });

  it('same form overlay', () => {
    expect(statesEqual(
      makeEntry({ overlay: { type: 'form', form: { type: 'credential', editing: 'aws' } } }),
      makeEntry({ overlay: { type: 'form', form: { type: 'credential', editing: 'aws' } } }),
    )).toBe(true);
  });

  it('different form overlay', () => {
    expect(statesEqual(
      makeEntry({ overlay: { type: 'form', form: { type: 'credential' } } }),
      makeEntry({ overlay: { type: 'form', form: { type: 'new-app' } } }),
    )).toBe(false);
  });

  it('same app-ui overlay', () => {
    const app = { id: 's1' } as any;
    expect(statesEqual(
      makeEntry({ overlay: { type: 'app-ui', app } }),
      makeEntry({ overlay: { type: 'app-ui', app } }),
    )).toBe(true);
  });

  it('different app-ui apps', () => {
    expect(statesEqual(
      makeEntry({ overlay: { type: 'app-ui', app: { id: 's1' } as any } }),
      makeEntry({ overlay: { type: 'app-ui', app: { id: 's2' } as any } }),
    )).toBe(false);
  });
});

describe('pushEntry', () => {
  it('returns null for duplicate entry', () => {
    const entry = makeEntry();
    expect(pushEntry([entry], 0, makeEntry())).toBeNull();
  });

  it('pushes new entry', () => {
    const result = pushEntry([makeEntry()], 0, makeEntry({ menuItem: 'settings' }));
    expect(result).not.toBeNull();
    expect(result!.stack).toHaveLength(2);
    expect(result!.cursor).toBe(1);
    expect(result!.stack[1].menuItem).toBe('settings');
  });

  it('truncates forward history when pushing from beginning', () => {
    const stack = [
      makeEntry(),
      makeEntry({ menuItem: 'settings' }),
      makeEntry({ menuItem: 'notifications' }),
    ];
    const result = pushEntry(stack, 0, makeEntry({ menuItem: 'apps' }));
    expect(result).not.toBeNull();
    expect(result!.stack).toHaveLength(2);
    expect(result!.cursor).toBe(1);
    expect(result!.stack[1].menuItem).toBe('apps');
  });

  it('truncates forward history when pushing from middle', () => {
    const stack = [
      makeEntry(),
      makeEntry({ menuItem: 'settings' }),
      makeEntry({ menuItem: 'notifications' }),
    ];
    const result = pushEntry(stack, 1, makeEntry({ menuItem: 'apps' }));
    expect(result).not.toBeNull();
    expect(result!.stack).toHaveLength(3);
    expect(result!.cursor).toBe(2);
  });

  it('pushes when overlay differs', () => {
    const result = pushEntry(
      [makeEntry()],
      0,
      makeEntry({ overlay: { type: 'file-preview', path: 'readme.md' } }),
    );
    expect(result).not.toBeNull();
    expect(result!.stack).toHaveLength(2);
  });
});

describe('back/forward simulation', () => {
  it('full navigation cycle', () => {
    let stack = [makeEntry()];
    let cursor = 0;

    // Navigate: files → settings → notifications
    let r = pushEntry(stack, cursor, makeEntry({ menuItem: 'settings' }));
    stack = r!.stack; cursor = r!.cursor;

    r = pushEntry(stack, cursor, makeEntry({ menuItem: 'notifications' }));
    stack = r!.stack; cursor = r!.cursor;
    expect(stack).toHaveLength(3);
    expect(cursor).toBe(2);

    // Back twice
    cursor--;
    expect(stack[cursor].menuItem).toBe('settings');
    cursor--;
    expect(stack[cursor].menuItem).toBe('files');

    // Forward
    cursor++;
    expect(stack[cursor].menuItem).toBe('settings');

    // Push new — truncates 'notifications'
    r = pushEntry(stack, cursor, makeEntry({ menuItem: 'apps' }));
    stack = r!.stack; cursor = r!.cursor;
    expect(stack).toHaveLength(3);
    expect(stack.map(e => e.menuItem)).toEqual(['files', 'settings', 'apps']);

    // Can't go forward anymore
    expect(cursor).toBe(stack.length - 1);
  });

  it('back to start then push replaces all forward history', () => {
    let stack = [makeEntry()];
    let cursor = 0;

    // Build up 3 entries
    for (const item of ['settings', 'notifications', 'apps'] as const) {
      const r = pushEntry(stack, cursor, makeEntry({ menuItem: item }));
      stack = r!.stack; cursor = r!.cursor;
    }
    expect(stack).toHaveLength(4);

    // Go all the way back
    cursor = 0;

    // Push new entry
    const r = pushEntry(stack, cursor, makeEntry({ menuItem: 'changes' }));
    stack = r!.stack; cursor = r!.cursor;
    expect(stack).toHaveLength(2);
    expect(stack.map(e => e.menuItem)).toEqual(['files', 'changes']);
  });

  it('duplicate push at cursor is no-op', () => {
    const stack = [makeEntry(), makeEntry({ menuItem: 'settings' })];
    const result = pushEntry(stack, 1, makeEntry({ menuItem: 'settings' }));
    expect(result).toBeNull();
  });

  it('caps stack at MAX_NAV_STACK and adjusts cursor', () => {
    let stack = [makeEntry()];
    let cursor = 0;

    // Push MAX_NAV_STACK entries (total = MAX_NAV_STACK + 1 including initial)
    for (let i = 0; i < MAX_NAV_STACK; i++) {
      const r = pushEntry(stack, cursor, makeEntry({ overlay: { type: 'file-preview', path: `file-${i}` } }));
      stack = r!.stack; cursor = r!.cursor;
    }

    // Stack should be capped at MAX_NAV_STACK
    expect(stack).toHaveLength(MAX_NAV_STACK);
    // Cursor should point to the last entry
    expect(cursor).toBe(MAX_NAV_STACK - 1);
    // Oldest entry was evicted
    const first = stack[0].overlay;
    const last = stack[MAX_NAV_STACK - 1].overlay;
    expect(first?.type === 'file-preview' && first.path).toBe('file-0');
    expect(last?.type === 'file-preview' && last.path).toBe(`file-${MAX_NAV_STACK - 1}`);
  });
});

describe('back button action selection (webview vs nav stack)', () => {
  function shouldUseWebviewBack(
    showUrlPreview: boolean,
    currentUrl: string | null,
    initialUrl: string | null,
  ): boolean {
    return showUrlPreview && currentUrl !== null && initialUrl !== null && currentUrl !== initialUrl;
  }

  it('back uses navBack when restored to URL entry with no webview navigation', () => {
    const result = shouldUseWebviewBack(true, 'https://example.com', 'https://example.com');
    expect(result).toBe(false);
  });

  it('back uses webview back when user navigated within webview', () => {
    const result = shouldUseWebviewBack(true, 'https://example.com/page2', 'https://example.com');
    expect(result).toBe(true);
  });

  it('back uses navBack when not in URL preview', () => {
    const result = shouldUseWebviewBack(false, null, null);
    expect(result).toBe(false);
  });

  it('back uses navBack after webview back returns to initial URL', () => {
    const result = shouldUseWebviewBack(true, 'https://example.com', 'https://example.com');
    expect(result).toBe(false);
  });

  it('back uses webview back when URL differs from initial (user navigated)', () => {
    const result = shouldUseWebviewBack(true, 'https://example.com/page2', 'https://example.com/');
    expect(result).toBe(true);
  });

  it('back uses navBack after redirect captured by initial URL update', () => {
    // After the first panel-url-changed event, webviewInitialUrl is updated
    // to the actual loaded URL (post-redirect). So both match.
    const redirectedUrl = 'https://www.example.com/';
    const result = shouldUseWebviewBack(true, redirectedUrl, redirectedUrl);
    expect(result).toBe(false);
  });
});

describe('same overlay with different menuItem is treated as duplicate', () => {
  it('re-opening same app from different menu context is a no-op', () => {
    const appOverlay = { type: 'app-ui' as const, app: { id: 'habit-tracker' } as any };
    const stack = [
      makeEntry(),
      makeEntry({ menuItem: 'apps', overlay: appOverlay }),
    ];
    const result = pushEntry(stack, 1, makeEntry({ menuItem: 'notifications', overlay: appOverlay }));
    expect(result).toBeNull();
  });

  it('opening a different app from different menu context still pushes', () => {
    const stack = [
      makeEntry(),
      makeEntry({ menuItem: 'apps', overlay: { type: 'app-ui', app: { id: 'app-a' } as any } }),
    ];
    const result = pushEntry(stack, 1, makeEntry({
      menuItem: 'notifications',
      overlay: { type: 'app-ui', app: { id: 'app-b' } as any },
    }));
    expect(result).not.toBeNull();
    expect(result!.stack).toHaveLength(3);
  });

  it('null overlay entries with different menuItems still push (menu navigation)', () => {
    const stack = [makeEntry({ menuItem: 'files' })];
    const result = pushEntry(stack, 0, makeEntry({ menuItem: 'settings' }));
    expect(result).not.toBeNull();
    expect(result!.stack).toHaveLength(2);
  });

  it('re-opening same file preview from different menu context is a no-op', () => {
    const overlay = { type: 'file-preview' as const, path: 'notes.md' };
    const stack = [
      makeEntry(),
      makeEntry({ menuItem: 'files', overlay }),
    ];
    const result = pushEntry(stack, 1, makeEntry({ menuItem: 'notifications', overlay }));
    expect(result).toBeNull();
  });
});

describe('overlay-only changes are accepted by pushEntry (regression guard)', () => {
  it('pushEntry accepts a url-preview-only change as a new entry', () => {
    const stack = [
      makeEntry(),
      makeEntry({ overlay: { type: 'url-preview', url: 'https://a.com' } }),
    ];
    const result = pushEntry(stack, 1, makeEntry({ overlay: { type: 'url-preview', url: 'https://b.com' } }));
    expect(result).not.toBeNull();
    expect(result!.stack).toHaveLength(3);
  });

  it('webview history back would re-push a previously visited URL', () => {
    const stack = [
      makeEntry(),
      makeEntry({ overlay: { type: 'url-preview', url: 'https://a.com' } }),
      makeEntry({ overlay: { type: 'url-preview', url: 'https://b.com' } }),
      makeEntry({ overlay: { type: 'url-preview', url: 'https://c.com' } }),
    ];
    const result = pushEntry(stack, 3, makeEntry({ overlay: { type: 'url-preview', url: 'https://b.com' } }));
    expect(result).not.toBeNull();
    expect(result!.stack).toHaveLength(5);
  });
});

// Mirrors the pure logic of `replaceNavState` in navigation.ts — used by the
// diff split-view sidebar so switching files inside the open panel keeps a
// single history slot with the latest selection winning.
function replaceEntry(stack: NavEntry[], cursor: number, entry: NavEntry): NavEntry[] | null {
  if (cursor < 0 || cursor >= stack.length) return null;
  const next = [...stack];
  next[cursor] = entry;
  return next;
}

describe('replaceEntry (latest-file-wins for diff split-view)', () => {
  it('overwrites the entry at the cursor without growing the stack', () => {
    const stack = [
      makeEntry(),
      makeEntry({ overlay: { type: 'file-preview', path: 'a.md' } }),
    ];
    const result = replaceEntry(stack, 1, makeEntry({ overlay: { type: 'file-preview', path: 'b.md' } }));
    expect(result).not.toBeNull();
    expect(result!).toHaveLength(2);
    expect((result![1].overlay as { path: string }).path).toBe('b.md');
  });

  it('chained replaces keep one slot — back goes to the entry before the panel opened', () => {
    let stack = [
      makeEntry(),
      makeEntry({ overlay: { type: 'file-preview', path: 'a.md' } }),
    ];
    for (const path of ['b.md', 'c.md', 'd.md']) {
      const next = replaceEntry(stack, 1, makeEntry({ overlay: { type: 'file-preview', path } }));
      stack = next!;
    }
    expect(stack).toHaveLength(2);
    expect((stack[1].overlay as { path: string }).path).toBe('d.md');
    // back from cursor=1 lands on the original entry, not on a previously
    // visited file inside the panel
    expect(stack[0].overlay).toBeNull();
  });

  it('refuses to replace when cursor is out of bounds', () => {
    const stack = [makeEntry()];
    expect(replaceEntry(stack, -1, makeEntry())).toBeNull();
    expect(replaceEntry(stack, 1, makeEntry())).toBeNull();
  });
});
