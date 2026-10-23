import { describe, it, expect } from 'vitest';
import { catalogScanFailure } from '../CatalogScanFailure';
import type { Loadable, MarketplaceCatalog } from '../../../store/types';

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

describe('catalogScanFailure', () => {
  it('says nothing about a healthy catalog', () => {
    expect(catalogScanFailure(loaded(), false)).toBeNull();
  });

  // A skeleton is already showing. A failure beside it would describe rows the
  // user cannot see.
  it('says nothing while the catalog is unloaded', () => {
    expect(catalogScanFailure({ status: 'not-loaded' }, false)).toBeNull();
    expect(catalogScanFailure({ status: 'loading' }, false)).toBeNull();
  });

  // A scan that could not run leaves the list frozen. Without this notice the
  // failure would be silent.
  it('carries the reason a scan could not run', () => {
    const catalog = loaded({ scan_error: 'read marketplace registry: boom' });
    expect(catalogScanFailure(catalog, false)).toEqual({
      reason: 'read marketplace registry: boom',
      retrying: false,
    });
  });

  // Hiding it during a retry would move the list on every scheduler pass.
  it('stays up while a new scan runs, and says it is retrying', () => {
    const catalog = loaded({ scan_error: 'read marketplace registry: boom' });
    expect(catalogScanFailure(catalog, true)).toEqual({
      reason: 'read marketplace registry: boom',
      retrying: true,
    });
  });
});
