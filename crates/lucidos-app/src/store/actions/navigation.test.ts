import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { PanelOverlay } from '../store';

// NavEntry and pure functions are tested directly without importing the
// navigation module (which pulls in store.ts → localStorage at module level).
// We duplicate the pure logic here to test it in isolation.

interface NavEntry {
  menuItem: string;
  settingsSubview: string;
  overlay: PanelOverlay;
  wipPreviewThreadId: string | null;
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
    overlaysEqual(a.overlay, b.overlay) &&
    a.wipPreviewThreadId === b.wipPreviewThreadId
  );
}

const MAX_NAV_STACK = 50;

function pushEntry(
  stack: NavEntry[],
  cursor: number,
  entry: NavEntry,
): { stack: NavEntry[]; cursor: number } | null {
  if (cursor < stack.length) {
    if (
      entry.overlay
      && overlaysEqual(entry.overlay, stack[cursor].overlay)
      && entry.wipPreviewThreadId === stack[cursor].wipPreviewThreadId
    ) return null;
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
    wipPreviewThreadId: null,
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

describe('wipPreviewThreadId is a nav-tracked axis', () => {
  const appOverlay = { type: 'app-ui' as const, app: { id: 'habit-tracker' } as any };

  it('statesEqual: same overlay, different wipPreviewThreadId is not equal', () => {
    expect(statesEqual(
      makeEntry({ overlay: appOverlay, wipPreviewThreadId: null }),
      makeEntry({ overlay: appOverlay, wipPreviewThreadId: 'thread-abc' }),
    )).toBe(false);
  });

  it('statesEqual: same overlay, same wipPreviewThreadId is equal', () => {
    expect(statesEqual(
      makeEntry({ overlay: appOverlay, wipPreviewThreadId: 'thread-abc' }),
      makeEntry({ overlay: appOverlay, wipPreviewThreadId: 'thread-abc' }),
    )).toBe(true);
  });

  it('pushEntry: toggling WIP on with the same app overlay pushes a new entry', () => {
    const stack = [
      makeEntry(),
      makeEntry({ menuItem: 'apps', overlay: appOverlay, wipPreviewThreadId: null }),
    ];
    const result = pushEntry(stack, 1, makeEntry({
      menuItem: 'apps',
      overlay: appOverlay,
      wipPreviewThreadId: 'thread-abc',
    }));
    expect(result).not.toBeNull();
    expect(result!.stack).toHaveLength(3);
    expect(result!.stack[2].wipPreviewThreadId).toBe('thread-abc');
  });

  it('pushEntry: toggling WIP off with the same app overlay pushes a new entry', () => {
    const stack = [
      makeEntry(),
      makeEntry({ menuItem: 'apps', overlay: appOverlay, wipPreviewThreadId: 'thread-abc' }),
    ];
    const result = pushEntry(stack, 1, makeEntry({
      menuItem: 'apps',
      overlay: appOverlay,
      wipPreviewThreadId: null,
    }));
    expect(result).not.toBeNull();
    expect(result!.stack).toHaveLength(3);
    expect(result!.stack[2].wipPreviewThreadId).toBeNull();
  });

  it('pushEntry: same overlay AND same WIP is still deduped', () => {
    const stack = [
      makeEntry(),
      makeEntry({ menuItem: 'apps', overlay: appOverlay, wipPreviewThreadId: 'thread-abc' }),
    ];
    const result = pushEntry(stack, 1, makeEntry({
      menuItem: 'notifications',
      overlay: appOverlay,
      wipPreviewThreadId: 'thread-abc',
    }));
    expect(result).toBeNull();
  });

  it('back/forward walks each WIP toggle', () => {
    // Simulate: open app → toggle WIP on → toggle WIP off → toggle WIP on
    let stack = [makeEntry({ menuItem: 'apps', overlay: appOverlay, wipPreviewThreadId: null })];
    let cursor = 0;
    for (const tid of ['thread-abc', null, 'thread-abc']) {
      const r = pushEntry(stack, cursor, makeEntry({
        menuItem: 'apps', overlay: appOverlay, wipPreviewThreadId: tid,
      }));
      stack = r!.stack; cursor = r!.cursor;
    }
    expect(stack.map(e => e.wipPreviewThreadId)).toEqual([null, 'thread-abc', null, 'thread-abc']);

    cursor--;
    expect(stack[cursor].wipPreviewThreadId).toBeNull();
    cursor--;
    expect(stack[cursor].wipPreviewThreadId).toBe('thread-abc');
    cursor--;
    expect(stack[cursor].wipPreviewThreadId).toBeNull();

    cursor += 3;
    expect(stack[cursor].wipPreviewThreadId).toBe('thread-abc');
  });
});

// Unlike the pure-logic suites above (which duplicate the stack math), this one
// exercises the REAL restoreState() path: seed the persisted nav stack in
// localStorage, import the navigation + store modules fresh, then trigger
// ensureInitialized() → restoreState() by reading a nav computed.
//
// Every entry restores its overlay, a PENDING form included. There are no
// transient navigations (ADR 0127). The guard that used to null a pending form
// claimed its staged request died with the page. Nothing of the sort is staged:
// the email and credential forms carry no request id at all, and the plugin
// ones are staged on the engine, which a reload never touches.
describe('restoreState restores every entry overlay on reload', () => {
  // Mirrors NAV_KEY in entityReferences.ts (the key restoreState reads/writes).
  const NAV_KEY = 'lucidos-nav-history';

  beforeEach(() => {
    vi.resetModules();
    localStorage.clear();
  });

  afterEach(() => {
    localStorage.clear();
  });

  /** Seed NAV_KEY with a single-entry stack whose cursor overlay is `overlay`
   *  (plus optional scalar nav fields), import navigation + store fresh, and
   *  trigger restore by reading a nav computed (which runs ensureInitialized).
   *  Returns the freshly-imported store module so the caller can assert on the
   *  signals restoreState mutated. */
  async function restoreWithOverlay(
    overlay: PanelOverlay,
    scalars: { menuItem?: string; settingsSubview?: string } = {},
  ) {
    const entry = {
      menuItem: scalars.menuItem ?? 'plugins',
      settingsSubview: scalars.settingsSubview ?? 'main',
      overlay,
      wipPreviewThreadId: null,
    };
    localStorage.setItem(NAV_KEY, JSON.stringify({ stack: [entry], cursor: 0 }));
    const nav = await import('./navigation');
    const store = await import('../store');
    // Reading a nav computed runs ensureInitialized() → restoreState(cursor entry).
    void nav.canGoBack.value;
    return store;
  }

  it('restores a PENDING plugin-uninstall, with its menuItem + settingsSubview', async () => {
    const store = await restoreWithOverlay(
      {
        type: 'form',
        form: {
          type: 'plugin-uninstall',
          request: {
            uninstall_id: 'u-live-uuid',
            plugin_id: 'habit-tracker',
            plugin_version: '1.0.0',
            plugin_name: 'Habit Tracker',
            files_present: ['data/apps/habit-tracker/manifest.json'],
            files_missing: [],
          },
        },
      },
      { menuItem: 'settings', settingsSubview: 'accounts' },
    );
    // The staged uninstall lives in the engine's `pending_uninstalls` map, which
    // a browser reload does not touch, so Confirm still resolves it.
    expect(store.activeInlineForm.value?.type).toBe('plugin-uninstall');
    expect(store.activeMenuItem.value).toBe('settings');
    expect(store.settingsSubview.value).toBe('accounts');
  });

  it('restores a PENDING plugin-install', async () => {
    const store = await restoreWithOverlay({
      type: 'form',
      form: {
        type: 'plugin-install',
        request: {
          install_id: 'i-live-uuid',
          source: 'git://example.com/habit-tracker',
          source_type: 'git',
          manifest: {},
          files: [],
          overwrites: [],
          plugin_id: 'habit-tracker',
          plugin_version: '1.0.0',
          plugin_name: 'Habit Tracker',
        },
      },
    });
    expect(store.activeInlineForm.value?.type).toBe('plugin-install');
  });

  it('restores a plugin-uninstall RECEIPT', async () => {
    const store = await restoreWithOverlay({
      type: 'form',
      form: {
        type: 'plugin-uninstall',
        request: {
          uninstall_id: 'u-spent-uuid',
          plugin_id: 'habit-tracker',
          plugin_version: '1.0.0',
          plugin_name: 'Habit Tracker',
          files_present: ['data/apps/habit-tracker/manifest.json'],
          files_missing: [],
        },
        // The files are already gone, so the panel is a record rather than a
        // request. It restores like every other entry, for a second reason.
        removed: {
          at: '2026-08-06T10:00:00.000Z',
          summary: 'Removed Habit Tracker',
          files_deleted: ['data/apps/habit-tracker/manifest.json'],
          files_missing: [],
        },
      },
    });
    expect(store.panelOverlay.value).not.toBeNull();
    expect(store.activeInlineForm.value?.type).toBe('plugin-uninstall');
  });

  it('restores a plugin-install RECEIPT', async () => {
    const store = await restoreWithOverlay({
      type: 'form',
      form: {
        type: 'plugin-install',
        request: {
          install_id: 'i-spent-uuid',
          source: 'git://example.com/habit-tracker',
          source_type: 'git',
          manifest: {},
          files: [],
          overwrites: [],
          plugin_id: 'habit-tracker',
          plugin_version: '1.0.0',
          plugin_name: 'Habit Tracker',
        },
        installed: {
          at: '2026-08-06T10:00:00.000Z',
          summary: 'Installed Habit Tracker',
          installed_files: ['data/apps/habit-tracker/manifest.json'],
        },
      },
    });
    expect(store.panelOverlay.value).not.toBeNull();
    expect(store.activeInlineForm.value?.type).toBe('plugin-install');
  });

  // The bug the user reported, in its reload form. The draft IS the request:
  // `EmailConfirmRequest` has no id, and Send posts the whole draft. So there is
  // nothing on either side to go stale.
  it('restores a PENDING email-confirm', async () => {
    const form = {
      type: 'email-confirm' as const,
      request: {
        to: ['someone@example.com'],
        subject: 'Hello',
        body: 'the draft, still unsent',
        account: 'work',
        from: 'me@example.com',
      },
    };
    const store = await restoreWithOverlay({ type: 'form', form });
    expect(store.panelOverlay.value).toEqual({ type: 'form', form });
  });

  it('restores a SENT email-confirm receipt', async () => {
    const form = {
      type: 'email-confirm' as const,
      request: {
        to: ['someone@example.com'],
        subject: 'Hello',
        body: 'the body that went out',
        account: 'work',
        from: 'me@example.com',
      },
      sentAt: '2026-07-29T09:15:00.000Z',
    };
    const store = await restoreWithOverlay({ type: 'form', form });
    expect(store.panelOverlay.value).toEqual({ type: 'form', form });
  });

  // `CredentialRequest` is a pre-fill descriptor and carries no id either. The
  // form is filled and saved locally, so it survives a reload like any other.
  it('restores an engine-prompted credential request (credential form WITH a request)', async () => {
    const overlay: PanelOverlay = {
      type: 'form',
      form: { type: 'credential', request: { service: 'example-service' } },
    };
    const store = await restoreWithOverlay(overlay);
    expect(store.panelOverlay.value).toEqual(overlay);
  });

  it('restores a Settings-driven credential edit (credential form WITHOUT a request)', async () => {
    const overlay: PanelOverlay = { type: 'form', form: { type: 'credential', editing: 'example-service' } };
    const store = await restoreWithOverlay(overlay);
    expect(store.panelOverlay.value).toEqual(overlay);
  });

  it('restores a persistent trigger form overlay', async () => {
    const overlay: PanelOverlay = { type: 'form', form: { type: 'trigger', triggerId: 'trigger-x' } };
    const store = await restoreWithOverlay(overlay);
    expect(store.panelOverlay.value).toEqual(overlay);
  });

  // A repo-encoded preview path must survive a reload byte-for-byte: restoreState
  // writes the overlay path straight through (no re-normalization), so ContentPane
  // re-parses it and re-mounts RepoFilePreview on the same repo file.
  it('restores a repo-encoded file preview verbatim', async () => {
    const overlay: PanelOverlay = {
      type: 'file-preview',
      path: 'repo:3f9c1b2e-0d44-4a71-9f6d-2e5b8c7a1d03:file:src/main/resources/transforms/x.jslt',
    };
    const store = await restoreWithOverlay(overlay);
    expect(store.panelOverlay.value).toEqual(overlay);
    expect(localStorage.getItem('file-preview-open')).toBe(overlay.path);
  });
});

// The reported bug, at the layer it lives in. A pending email confirm opened on
// top of Settings, Access. Walking history back to it showed Access instead: the
// overlay was nulled while `menuItem` and `settingsSubview` still applied, so
// the user landed on the bare panel underneath. A SENT confirm walked correctly,
// which is what isolated the guard.
describe('walking history reaches a pending email-confirm', () => {
  const NAV_KEY = 'lucidos-nav-history';

  const PENDING_CONFIRM: PanelOverlay = {
    type: 'form',
    form: {
      type: 'email-confirm',
      request: {
        to: ['someone@example.com'],
        subject: 'Quarterly numbers',
        body: 'the draft, still unsent',
        account: 'work',
        from: 'me@example.com',
      },
    },
  };

  /** The stack the report describes: Settings/Access bare, the confirm opened on
   *  top of it, then the user moves on to Files. Cursor starts at Files. */
  const STACK = [
    { menuItem: 'settings', settingsSubview: 'accounts', overlay: null, wipPreviewThreadId: null },
    { menuItem: 'settings', settingsSubview: 'accounts', overlay: PENDING_CONFIRM, wipPreviewThreadId: null },
    { menuItem: 'files', settingsSubview: 'main', overlay: null, wipPreviewThreadId: null },
  ];

  beforeEach(() => {
    vi.resetModules();
    localStorage.clear();
  });

  afterEach(() => {
    localStorage.clear();
  });

  async function seeded(cursor: number) {
    localStorage.setItem(NAV_KEY, JSON.stringify({ stack: STACK, cursor }));
    const nav = await import('./navigation');
    const store = await import('../store');
    void nav.canGoBack.value;
    return { nav, store };
  }

  it('navBack lands on the confirm, not the panel underneath', async () => {
    const { nav, store } = await seeded(2);
    nav.navBack();
    expect(store.panelOverlay.value).toEqual(PENDING_CONFIRM);
    expect(store.activeMenuItem.value).toBe('settings');
    expect(store.settingsSubview.value).toBe('accounts');
  });

  it('navForward lands on the confirm', async () => {
    const { nav, store } = await seeded(0);
    nav.navForward();
    expect(store.panelOverlay.value).toEqual(PENDING_CONFIRM);
  });

  // The path the user actually took: the nav-history popover, which jumps to an
  // absolute index rather than stepping.
  it('navGoTo lands on the confirm', async () => {
    const { nav, store } = await seeded(2);
    nav.navGoTo(1);
    expect(store.panelOverlay.value).toEqual(PENDING_CONFIRM);
  });

  it('walking away and back returns to the confirm', async () => {
    const { nav, store } = await seeded(1);
    nav.navForward();
    expect(store.panelOverlay.value).toBeNull();
    nav.navBack();
    expect(store.panelOverlay.value).toEqual(PENDING_CONFIRM);
  });

  // Nothing sanitizes the persisted stack at load: no entry is stripped, none is
  // dropped, and the cursor is exactly what was written.
  it('leaves the persisted stack and cursor untouched at load', async () => {
    const { nav } = await seeded(2);
    expect(nav.navHistory.value.stack).toEqual(STACK);
    expect(nav.navHistory.value.cursor).toBe(2);
  });
});
