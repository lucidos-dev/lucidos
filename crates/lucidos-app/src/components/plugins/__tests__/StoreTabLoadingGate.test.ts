import { describe, it, expect } from 'vitest';
import { pluginRowsSettled } from '../StoreTab';
import type { Loadable } from '../../../store/types';

const NOT_LOADED: Loadable<unknown> = { status: 'not-loaded' };
const LOADING: Loadable<unknown> = { status: 'loading' };
const LOADED: Loadable<unknown> = { status: 'loaded', data: [] };
const FAILED: Loadable<unknown> = { status: 'failed', error: 'boom' };

// pluginRowsSettled(catalog, installed) gates the Plugins list loading skeleton.
//
// The regression it pins: in Installed mode the skeleton used to gate ONLY on the
// installed projection (a fast local disk scan). But the rows are enriched from
// the catalog — descriptions, categories, the category-filter bar, and the
// update_available status — and the catalog scan (which clones marketplace repos)
// is far slower. Releasing the skeleton the instant the installed list landed
// painted bare orphan rows that then visibly REORGANIZED when the catalog arrived
// ("partial load then reorganizing, no skeleton"). The gate must wait for BOTH
// sources to settle so the first painted content is already in its final shape.
describe('pluginRowsSettled — the loading-skeleton gate for the Plugins list', () => {
  it('stays loading while the catalog is still loading even though installed is loaded (the bug)', () => {
    expect(pluginRowsSettled(LOADING, LOADED)).toBe(false);
  });

  it('stays loading while the installed list is still loading even though the catalog is loaded', () => {
    expect(pluginRowsSettled(LOADED, LOADING)).toBe(false);
  });

  it('is settled once both sources are loaded (a single clean render)', () => {
    expect(pluginRowsSettled(LOADED, LOADED)).toBe(true);
  });

  it('treats a FAILED source as settled so a catalog-scan failure never hangs the skeleton (best-effort Installed view)', () => {
    expect(pluginRowsSettled(FAILED, LOADED)).toBe(true);
    expect(pluginRowsSettled(LOADED, FAILED)).toBe(true);
    expect(pluginRowsSettled(FAILED, FAILED)).toBe(true);
  });

  it('is not settled on a cold load before either source resolves', () => {
    expect(pluginRowsSettled(NOT_LOADED, NOT_LOADED)).toBe(false);
    expect(pluginRowsSettled(NOT_LOADED, LOADED)).toBe(false);
    expect(pluginRowsSettled(LOADED, NOT_LOADED)).toBe(false);
  });
});
