import { describe, it, expect } from 'vitest';
import STORE_TAB from '../StoreTab.tsx?raw';
import DISPATCHER from '../../../store/actions/entityReferences.ts?raw';

const BANNED: Array<[string, RegExp]> = [
  ['an interval', /setInterval\s*\(/],
  ['a visibility listener', /'visibilitychange'/],
  ['a focus listener', /'focus'\s*,/],
  ['a refresh constant for a timer', /CATALOG_REFRESH_MS/],
];

describe('StoreTab', () => {
  it.each(BANNED)('answers a catalog change with no %s', (what, pattern) => {
    expect(pattern.test(STORE_TAB), `StoreTab.tsx still carries ${what}: ${pattern}`).toBe(false);
  });

  it('still refreshes both sources when the panel opens', () => {
    expect(STORE_TAB).toMatch(/void refreshPluginCatalog\(\);/);
    expect(STORE_TAB).toMatch(/void loadInstalledPlugins\(\);/);
  });
});

describe('the marketplace SSE arm the panel relies on', () => {
  it('reloads the catalog on both marketplace frames', () => {
    expect(DISPATCHER).toMatch(/case 'PluginMarketplaceRegistered':/);
    expect(DISPATCHER).toMatch(/case 'PluginMarketplaceRemoved':/);
    expect(DISPATCHER).toMatch(/refreshPluginCatalogAfterMutation\(\)/);
  });
});
