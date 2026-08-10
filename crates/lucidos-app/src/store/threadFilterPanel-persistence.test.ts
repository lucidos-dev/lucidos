/**
 * The thread drawer showing its FILTER PANEL is a state of the drawer, and it
 * MUST survive a page reload the way every other drawer state does (the
 * selected view in `altView-persistence.test.ts`, the channel selection, the
 * drawer's own open/closed, its width). Without it, a reload drops the user out
 * of the filters they were editing and back onto the list, which on mobile is
 * the whole threads pane changing under them.
 *
 * Restoration: `threadFilterPanel.ts` reads the key on init and, when it
 * restores open, registers on the Escape stack right there. That registration
 * is the half a plain persisted boolean would miss: the panel is not an
 * `<Overlay>`, so nothing else puts it on the stack, and a panel restored open
 * without it would ignore Escape.
 *
 * Persistence: every open/close writes the key, clearing it for the default
 * (closed) so a pristine state restores pristine.
 *
 * Restore-time behavior needs a fresh module instance per case, hence
 * `vi.resetModules()` + dynamic import (the signal is initialized at module
 * load). The plain state/registration contract is in `threadFilterPanel.test.ts`.
 */
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
    // And that entry closes the restored panel, rather than being an inert id.
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
