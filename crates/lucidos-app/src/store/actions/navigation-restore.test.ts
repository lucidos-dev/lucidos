import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const revealContentPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane }));

const NAV_KEY = 'lucidos-nav-history';

beforeEach(() => {
  vi.resetModules();
  localStorage.clear();
  revealContentPane.mockClear();
});

afterEach(() => {
  localStorage.clear();
});

describe('restoring the saved history on page load', () => {
  it('does not force the content pane open', async () => {
    localStorage.setItem(NAV_KEY, JSON.stringify({
      stack: [{ menuItem: 'plugins', settingsSubview: 'main', overlay: null, wipPreviewThreadId: null }],
      cursor: 0,
    }));
    const nav = await import('./navigation');
    const store = await import('../store');
    void nav.canGoBack.value;
    expect(store.activeMenuItem.value).toBe('plugins');
    expect(revealContentPane).not.toHaveBeenCalled();
  });

  it('migrates a retired settings subview on an entry already in overlay shape', async () => {
    localStorage.setItem(NAV_KEY, JSON.stringify({
      stack: [{ menuItem: 'settings', settingsSubview: 'repositories', overlay: null, wipPreviewThreadId: null }],
      cursor: 0,
    }));
    const nav = await import('./navigation');
    const store = await import('../store');
    void nav.canGoBack.value;
    expect(store.settingsSubview.value).toBe('coding-agents');
  });

  it('restores the WIP preview after the overlay, so the WIP effect keeps it', async () => {
    localStorage.setItem(NAV_KEY, JSON.stringify({
      stack: [{
        menuItem: 'apps',
        settingsSubview: 'main',
        overlay: { type: 'app-ui', app: { id: 'app-b', name: 'App B' } },
        wipPreviewThreadId: 'thread-wip',
      }],
      cursor: 0,
    }));
    const nav = await import('./navigation');
    const store = await import('../store');
    await import('./wipPreview');
    store.panelOverlay.value = { type: 'app-ui', app: { id: 'app-a', name: 'App A' } } as never;
    store.focusedThreadId.value = 'thread-wip';
    store.threadsLoaded.value = true;
    store.threadMap.value = new Map([
      ['thread-wip', { id: 'thread-wip', meta: { codingAgentFolder: '/repos/app-b' } }],
    ]) as never;
    void nav.canGoBack.value;
    expect(store.wipPreviewThreadId.value).toBe('thread-wip');
    expect(store.currentApp.value?.id).toBe('app-b');
  });
});
