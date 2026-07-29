// Hydration helper for the pinnedApps offline cache: empty/valid JSON → loaded;
// invalid JSON → failed (must not silently collapse to []).
import { describe, it, expect, beforeEach } from 'vitest';
import { hydratePinnedAppsFromStorage } from './pinnedApps';
import type { PinnedAppEntry } from '../types';
import type { Loadable } from '../types';

describe('hydratePinnedAppsFromStorage', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('returns loaded with empty data when key is absent', () => {
    const out: Loadable<PinnedAppEntry[]> = hydratePinnedAppsFromStorage();
    expect(out.status).toBe('loaded');
    if (out.status === 'loaded') expect(out.data).toEqual([]);
  });

  it('returns loaded with parsed entries when key holds valid JSON', () => {
    localStorage.setItem(
      'pinned_apps',
      JSON.stringify([{ app_id: 'habit-tracker' }, { app_id: 'ledger' }]),
    );
    const out = hydratePinnedAppsFromStorage();
    expect(out.status).toBe('loaded');
    if (out.status === 'loaded') {
      expect(out.data).toEqual([{ app_id: 'habit-tracker' }, { app_id: 'ledger' }]);
    }
  });

  it('returns failed when key holds invalid JSON', () => {
    // Today (pre-fix) the inline IIFE swallowed the parse error and returned [].
    // Loadable contract: surface the error so the user sees a real failure
    // instead of an empty list that masks corruption.
    localStorage.setItem('pinned_apps', '{{not json');
    const out = hydratePinnedAppsFromStorage();
    expect(out.status).toBe('failed');
    if (out.status === 'failed') {
      expect(out.error).toBeTruthy();
    }
  });
});
