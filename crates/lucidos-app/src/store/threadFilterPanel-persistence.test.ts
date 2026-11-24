import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const KEY = 'lucidos-thread-filter-panel-open';

describe('thread filter panel persistence (the drawer remembers it is showing filters)', () => {
  beforeEach(() => {
    vi.resetModules();
    localStorage.removeItem(KEY);
  });

  afterEach(() => {
    localStorage.removeItem(KEY);
  });

  it('initializes closed when nothing is stored', async () => {
    const { threadFilterPanelOpen } = await import('./threadFilterPanel');
    expect(threadFilterPanelOpen.value).toBe(false);
  });

  it('restores the panel open from localStorage on init', async () => {
    localStorage.setItem(KEY, 'true');
    const { threadFilterPanelOpen } = await import('./threadFilterPanel');
    expect(threadFilterPanelOpen.value).toBe(true);
  });

  it('treats an unknown stored value as closed', async () => {
    localStorage.setItem(KEY, 'yes');
    const { threadFilterPanelOpen } = await import('./threadFilterPanel');
    expect(threadFilterPanelOpen.value).toBe(false);
  });

  it('registers a panel restored open on the Escape stack', async () => {
    localStorage.setItem(KEY, 'true');
    const { threadFilterPanelOpen } = await import('./threadFilterPanel');
    const { overlayStack, dismissTopOverlay } = await import('./overlayStack');
    expect(overlayStack.value.map(e => e.id)).toEqual(['thread-filter-panel']);
    expect(dismissTopOverlay()).toBe(true);
    expect(threadFilterPanelOpen.value).toBe(false);
    expect(localStorage.getItem(KEY)).toBeNull();
  });

  it('leaves the Escape stack empty when the panel restores closed', async () => {
    await import('./threadFilterPanel');
    const { overlayStack } = await import('./overlayStack');
    expect(overlayStack.value).toHaveLength(0);
  });

  it('persists opening and clears the key on close', async () => {
    const { openThreadFilterPanel, closeThreadFilterPanel } = await import('./threadFilterPanel');
    openThreadFilterPanel();
    expect(localStorage.getItem(KEY)).toBe('true');
    closeThreadFilterPanel();
    expect(localStorage.getItem(KEY)).toBeNull();
  });

  it('persists through the toggle, whichever way it goes', async () => {
    const { toggleThreadFilterPanel } = await import('./threadFilterPanel');
    toggleThreadFilterPanel();
    expect(localStorage.getItem(KEY)).toBe('true');
    toggleThreadFilterPanel();
    expect(localStorage.getItem(KEY)).toBeNull();
  });
});
