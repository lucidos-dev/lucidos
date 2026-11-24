import { describe, it, expect } from 'vitest';
import {
  availableMarketplaces,
  resolveActiveMarketplace,
  matchesFilters,
  pluginDeepLinkCouldBeFiltered,
} from '../StoreTab';
import type { MarketplacePlugin } from '../../../store/types';

function row(over: Partial<MarketplacePlugin>): MarketplacePlugin {
  return {
    marketplace_id: 'mkt-a',
    marketplace_name: 'Alpha',
    id: 'p',
    name: 'Plugin',
    description: '',
    version: '1.0.0',
    source: 'https://example.com/p',
    manifest: {},
    content: [],
    categories: [],
    files_count: 1,
    engine_compatible: true,
    status: 'available',
    media: { screenshots: [], videos: [], problems: [] },
    ...over,
  };
}

const ALPHA = row({ marketplace_id: 'mkt-a', marketplace_name: 'Alpha', id: 'a' });
const ZULU = row({ marketplace_id: 'mkt-z', marketplace_name: 'Zulu', id: 'z' });
const ORPHAN = row({
  marketplace_id: 'installed:ghost',
  marketplace_name: 'https://example.com/ghost.git',
  id: 'ghost',
  status: 'installed',
});

describe('availableMarketplaces: the pills the catalog list offers', () => {
  it('dedupes by id and sorts by name', () => {
    const rows = [ZULU, ALPHA, row({ marketplace_id: 'mkt-a', marketplace_name: 'Alpha', id: 'a2' })];
    expect(availableMarketplaces(rows)).toEqual([
      { id: 'mkt-a', name: 'Alpha' },
      { id: 'mkt-z', name: 'Zulu' },
    ]);
  });

  it('never offers an orphan row as a marketplace', () => {
    expect(availableMarketplaces([ALPHA, ORPHAN])).toEqual([{ id: 'mkt-a', name: 'Alpha' }]);
  });

  it('counts real marketplaces only, orphans excluded', () => {
    expect(availableMarketplaces([ALPHA, ALPHA, ORPHAN]).length).toBe(1);
    expect(availableMarketplaces([ALPHA, ZULU]).length).toBe(2);
    expect(availableMarketplaces([ORPHAN])).toEqual([]);
  });
});

describe('resolveActiveMarketplace: the stale-selection guard', () => {
  const available = [
    { id: 'mkt-a', name: 'Alpha' },
    { id: 'mkt-z', name: 'Zulu' },
  ];

  it('resolves a live selection to its pill', () => {
    expect(resolveActiveMarketplace('mkt-z', available)).toEqual({ id: 'mkt-z', name: 'Zulu' });
  });

  it('falls back to All when the selected marketplace is gone', () => {
    expect(resolveActiveMarketplace('mkt-gone', available)).toBeNull();
  });

  it('is All with nothing selected', () => {
    expect(resolveActiveMarketplace(null, available)).toBeNull();
  });

  it('keeps a lone surviving marketplace selected, because the dropdown still shows it', () => {
    expect(resolveActiveMarketplace('mkt-z', [{ id: 'mkt-z', name: 'Zulu' }])).toEqual({
      id: 'mkt-z',
      name: 'Zulu',
    });
  });

  it('is All when the rows carry no marketplace at all', () => {
    expect(resolveActiveMarketplace('mkt-z', [])).toBeNull();
  });
});

describe('matchesFilters: query AND marketplace AND category', () => {
  const plugin = row({
    marketplace_id: 'mkt-a',
    marketplace_name: 'Alpha',
    name: 'Budget tracker',
    categories: ['finance', 'productivity'],
  });

  it('keeps a row every active filter agrees on', () => {
    expect(matchesFilters(plugin, 'budget', 'mkt-a', 'finance')).toBe(true);
  });

  it('drops a row on the marketplace alone, with query and category matching', () => {
    expect(matchesFilters(plugin, 'budget', 'mkt-z', 'finance')).toBe(false);
  });

  it('drops a row on the category alone, with query and marketplace matching', () => {
    expect(matchesFilters(plugin, 'budget', 'mkt-a', 'health')).toBe(false);
  });

  it('drops a row on the query alone, with marketplace and category matching', () => {
    expect(matchesFilters(plugin, 'ledger', 'mkt-a', 'finance')).toBe(false);
  });

  it('keeps every row when nothing is filtering', () => {
    expect(matchesFilters(plugin, '', null, null)).toBe(true);
    expect(matchesFilters(ORPHAN, '', null, null)).toBe(true);
  });

  it('drops the orphan rows as soon as a marketplace is picked', () => {
    expect(matchesFilters(ORPHAN, '', 'mkt-a', null)).toBe(false);
  });
});

describe('pluginDeepLinkCouldBeFiltered: when a missing row proves nothing', () => {
  it('holds the target while any one filter is on', () => {
    expect(pluginDeepLinkCouldBeFiltered('budget', null, null)).toBe(true);
    expect(pluginDeepLinkCouldBeFiltered('', 'mkt-a', null)).toBe(true);
    expect(pluginDeepLinkCouldBeFiltered('', null, 'finance')).toBe(true);
  });

  it('counts the marketplace filter, which outlives the panel', () => {
    expect(pluginDeepLinkCouldBeFiltered('', 'mkt-a', null)).toBe(true);
  });

  it('gives up only with every filter off', () => {
    expect(pluginDeepLinkCouldBeFiltered('', null, null)).toBe(false);
    expect(pluginDeepLinkCouldBeFiltered('   ', null, null)).toBe(false);
  });
});
