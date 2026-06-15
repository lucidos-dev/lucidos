/**
 * The alternate drawer views (needs-attention / drafts) MUST remember which one
 * is active across a page reload. Without persistence, toggling into the
 * needs-attention view → reloading drops you back to the normal drawer (the
 * toggle's active state is lost), even though the icon + count reappear on their
 * own (they're recomputed from the rehydrated threadMap).
 *
 * Restoration: `store.ts` reads 'lucidos-alt-view' from localStorage on init.
 * Persistence: the two toggles write it back (via persistAltView) on every flip.
 * Mutual exclusivity: at most one view is active, encoded as a single tri-state
 * key, so restore can never resurrect both.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const KEY = 'lucidos-alt-view';

describe('alternate-view persistence (drawer remembers the active view)', () => {
  beforeEach(() => {
    vi.resetModules();
    localStorage.removeItem(KEY);
  });

  afterEach(() => {
    localStorage.removeItem(KEY);
  });

  it('initializes with both views off when nothing is stored', async () => {
    const { attentionViewActive, draftsViewActive } = await import('./store');
    expect(attentionViewActive.value).toBe(false);
    expect(draftsViewActive.value).toBe(false);
  });

  it('restores the needs-attention view from localStorage on init', async () => {
    localStorage.setItem(KEY, 'attention');
    const { attentionViewActive, draftsViewActive } = await import('./store');
    expect(attentionViewActive.value).toBe(true);
    expect(draftsViewActive.value).toBe(false);
  });

  it('restores the drafts view from localStorage on init', async () => {
    localStorage.setItem(KEY, 'drafts');
    const { attentionViewActive, draftsViewActive } = await import('./store');
    expect(draftsViewActive.value).toBe(true);
    expect(attentionViewActive.value).toBe(false);
  });

  it('treats an unknown stored value as no active view', async () => {
    localStorage.setItem(KEY, 'something-else');
    const { attentionViewActive, draftsViewActive } = await import('./store');
    expect(attentionViewActive.value).toBe(false);
    expect(draftsViewActive.value).toBe(false);
  });

  it('persists the needs-attention view when toggled on', async () => {
    const { toggleAttentionView } = await import('./store');
    toggleAttentionView();
    expect(localStorage.getItem(KEY)).toBe('attention');
  });

  it('persists the drafts view when toggled on', async () => {
    const { toggleDraftsView } = await import('./store');
    toggleDraftsView();
    expect(localStorage.getItem(KEY)).toBe('drafts');
  });

  it('clears the key when the active view is toggled back off', async () => {
    const { toggleAttentionView } = await import('./store');
    toggleAttentionView();
    expect(localStorage.getItem(KEY)).toBe('attention');
    toggleAttentionView();
    expect(localStorage.getItem(KEY)).toBeNull();
  });

  it('switching from drafts to needs-attention persists the new view (mutual exclusion)', async () => {
    const { toggleDraftsView, toggleAttentionView, attentionViewActive, draftsViewActive } =
      await import('./store');
    toggleDraftsView();
    expect(localStorage.getItem(KEY)).toBe('drafts');
    toggleAttentionView();
    expect(attentionViewActive.value).toBe(true);
    expect(draftsViewActive.value).toBe(false);
    expect(localStorage.getItem(KEY)).toBe('attention');
  });
});
