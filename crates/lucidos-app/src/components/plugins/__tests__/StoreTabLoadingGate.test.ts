import { describe, it, expect } from 'vitest';
import { emptyCatalogMessage, pluginRowsSettled } from '../StoreTab';
import { loadingIfFresh } from '../../../store/types';
import type { Loadable } from '../../../store/types';

const NOT_LOADED: Loadable<unknown> = { status: 'not-loaded' };
const LOADING: Loadable<unknown> = { status: 'loading' };
const LOADED: Loadable<unknown> = { status: 'loaded', data: [] };
const FAILED: Loadable<unknown> = { status: 'failed', error: 'boom' };

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

describe('a refresh never covers rows the panel already holds', () => {
  it('stays settled while an already-loaded catalog re-fetches', () => {
    expect(pluginRowsSettled(loadingIfFresh(LOADED), loadingIfFresh(LOADED))).toBe(true);
  });

  it('still shows the skeleton on a genuinely cold load', () => {
    expect(pluginRowsSettled(loadingIfFresh(NOT_LOADED), loadingIfFresh(NOT_LOADED))).toBe(false);
  });

  it('shows the skeleton when only one source is warm', () => {
    expect(pluginRowsSettled(loadingIfFresh(NOT_LOADED), loadingIfFresh(LOADED))).toBe(false);
  });
});

describe('emptyCatalogMessage: an empty catalog list mid-scan', () => {
  it('says the scan is running rather than claiming there are no plugins', () => {
    expect(emptyCatalogMessage(true)).toBe('Scanning marketplaces…');
  });

  it('says there are none once nothing is scanning', () => {
    expect(emptyCatalogMessage(false)).toBe('No plugins found.');
  });
});
