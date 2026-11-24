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

  it('says nothing while the catalog is unloaded', () => {
    expect(catalogScanFailure({ status: 'not-loaded' }, false)).toBeNull();
    expect(catalogScanFailure({ status: 'loading' }, false)).toBeNull();
  });

  it('carries the reason a scan could not run', () => {
    const catalog = loaded({ scan_error: 'read marketplace registry: boom' });
    expect(catalogScanFailure(catalog, false)).toEqual({
      reason: 'read marketplace registry: boom',
      retrying: false,
    });
  });

  it('stays up while a new scan runs, and says it is retrying', () => {
    const catalog = loaded({ scan_error: 'read marketplace registry: boom' });
    expect(catalogScanFailure(catalog, true)).toEqual({
      reason: 'read marketplace registry: boom',
      retrying: true,
    });
  });
});
