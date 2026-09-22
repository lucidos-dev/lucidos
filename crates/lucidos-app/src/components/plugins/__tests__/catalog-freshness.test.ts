import { describe, it, expect } from 'vitest';
import { catalogFreshness } from '../CatalogFreshness';
import type { Loadable, MarketplaceCatalog } from '../../../store/types';

const NOW = new Date('2026-09-22T12:00:00Z');

function loaded(over: Partial<MarketplaceCatalog> = {}): Loadable<MarketplaceCatalog> {
  return {
    status: 'loaded',
    data: {
      marketplaces: [],
      plugins: [],
      errors: [],
      scanned_at: '2026-09-22T11:57:00Z',
      scanning: false,
      scan_error: null,
      ...over,
    },
  };
}

describe('catalogFreshness', () => {
  // A skeleton is already saying "nothing here yet". An age beside it would be
  // describing rows the user cannot see.
  it('draws nothing while the catalog is unloaded', () => {
    expect(catalogFreshness({ status: 'not-loaded' }, false, NOW)).toBeNull();
    expect(catalogFreshness({ status: 'loading' }, true, NOW)).toBeNull();
  });

  it('reports the age of the scan behind the rows', () => {
    const state = catalogFreshness(loaded(), false, NOW);
    expect(state?.label).toBe('Updated 3 minutes ago');
    expect(state?.busy).toBe(false);
  });

  // The whole point of the cue: rows stay on screen and the panel says a
  // refresh is under way, rather than covering them with a skeleton.
  it('says it is updating while a scan runs', () => {
    const state = catalogFreshness(loaded(), true, NOW);
    expect(state?.label).toBe('Updating…');
    expect(state?.busy).toBe(true);
  });

  // The scanning flag wins over the age, so a scan that starts does not leave
  // the panel quoting a timestamp that is about to change.
  it('prefers the updating state over a stale age', () => {
    const old = loaded({ scanned_at: '2026-09-20T12:00:00Z' });
    expect(catalogFreshness(old, true, NOW)?.label).toBe('Updating…');
  });

  // A scan that could not run at all leaves the list frozen. Reporting only its
  // age would be a silent failure: the number keeps rising and says nothing.
  it('names a scan that could not run, and carries the reason', () => {
    const state = catalogFreshness(loaded({ scan_error: 'read marketplace registry: boom' }), false, NOW);
    expect(state?.label).toBe('Update failed');
    expect(state?.tooltip).toContain('read marketplace registry: boom');
  });

  it('says so when nothing has ever been scanned', () => {
    expect(catalogFreshness(loaded({ scanned_at: null }), false, NOW)?.label).toBe('Not checked yet');
  });

  // An unparseable stamp must read as "no scan", never as "Invalid Date".
  it('treats a stamp it cannot read as no scan at all', () => {
    const state = catalogFreshness(loaded({ scanned_at: 'not a date' }), false, NOW);
    expect(state?.label).toBe('Not checked yet');
  });
});
