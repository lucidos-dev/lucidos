/**
 * The drawer view selection (all / attention / review / in-flight / drafts) MUST
 * survive a page reload. Without persistence, switching to e.g. the review view → reloading
 * drops you back to the default sectioned list, even though the selector icon +
 * counts reappear on their own (recomputed from the rehydrated threadMap).
 *
 * Restoration: `store.ts` reads 'lucidos-alt-view' from localStorage on init
 * (`restoreDrawerView`). Persistence: `setDrawerView` writes it back on every
 * pick — clearing the key for the default `all` so a pristine state restores to
 * the default. Legacy `'attention'`/`'drafts'` values still restore, the view's
 * old name `'running'` restores as `'in-flight'`, and the retired `'none'` and
 * any unknown value fall back to `all`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const KEY = 'lucidos-alt-view';

describe('drawer view persistence (drawer remembers the selected view)', () => {
  beforeEach(() => {
    vi.resetModules();
    localStorage.removeItem(KEY);
  });

  afterEach(() => {
    localStorage.removeItem(KEY);
  });

  it('initializes with the default `all` view when nothing is stored', async () => {
    const { drawerView } = await import('./store');
    expect(drawerView.value).toBe('all');
  });

  it('restores the needs-attention view from localStorage on init', async () => {
    localStorage.setItem(KEY, 'attention');
    const { drawerView } = await import('./store');
    expect(drawerView.value).toBe('attention');
  });

  it('restores the review view from localStorage on init', async () => {
    localStorage.setItem(KEY, 'review');
    const { drawerView } = await import('./store');
    expect(drawerView.value).toBe('review');
  });

  it('restores the in-flight view from localStorage on init', async () => {
    localStorage.setItem(KEY, 'in-flight');
    const { drawerView } = await import('./store');
    expect(drawerView.value).toBe('in-flight');
  });

  it('restores the old `running` value as the in-flight view', async () => {
    localStorage.setItem(KEY, 'running');
    const { drawerView } = await import('./store');
    expect(drawerView.value).toBe('in-flight');
  });

  it('restores the drafts view from localStorage on init', async () => {
    localStorage.setItem(KEY, 'drafts');
    const { drawerView } = await import('./store');
    expect(drawerView.value).toBe('drafts');
  });

  it('treats the retired `none` value as the default `all` view', async () => {
    localStorage.setItem(KEY, 'none');
    const { drawerView } = await import('./store');
    expect(drawerView.value).toBe('all');
  });

  it('treats an unknown stored value as the default `all` view', async () => {
    localStorage.setItem(KEY, 'something-else');
    const { drawerView } = await import('./store');
    expect(drawerView.value).toBe('all');
  });

  it('persists each non-default view when selected', async () => {
    const { setDrawerView, drawerView } = await import('./store');
    setDrawerView('attention');
    expect(drawerView.value).toBe('attention');
    expect(localStorage.getItem(KEY)).toBe('attention');
    setDrawerView('review');
    expect(drawerView.value).toBe('review');
    expect(localStorage.getItem(KEY)).toBe('review');
    setDrawerView('in-flight');
    expect(drawerView.value).toBe('in-flight');
    expect(localStorage.getItem(KEY)).toBe('in-flight');
    setDrawerView('drafts');
    expect(localStorage.getItem(KEY)).toBe('drafts');
  });

  it('clears the key when the default `all` view is selected', async () => {
    const { setDrawerView } = await import('./store');
    setDrawerView('review');
    expect(localStorage.getItem(KEY)).toBe('review');
    setDrawerView('all');
    expect(localStorage.getItem(KEY)).toBeNull();
  });

  it('switching views keeps exactly one active (the latest pick)', async () => {
    const { setDrawerView, drawerView } = await import('./store');
    setDrawerView('drafts');
    expect(localStorage.getItem(KEY)).toBe('drafts');
    setDrawerView('attention');
    expect(drawerView.value).toBe('attention');
    expect(localStorage.getItem(KEY)).toBe('attention');
  });
});
