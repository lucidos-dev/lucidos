import { describe, it, expect } from 'vitest';
import {
  availableMarketplaces,
  resolveActiveMarketplace,
  matchesFilters,
  pluginDeepLinkCouldBeFiltered,
} from '../StoreTab';
import type { MarketplacePlugin } from '../../../store/types';

/** A catalog row with only the fields the three filter helpers read. */
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
    status: 'available',
    ...over,
  };
}

const ALPHA = row({ marketplace_id: 'mkt-a', marketplace_name: 'Alpha', id: 'a' });
const ZULU = row({ marketplace_id: 'mkt-z', marketplace_name: 'Zulu', id: 'z' });
// An installed plugin whose marketplace is no longer registered. `orphanRow`
// builds exactly this shape: a synthetic id, and the plugin's source sitting in
// the marketplace_name slot.
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
    // The orphan's marketplace is gone, so its id matches nothing and its name
    // slot holds a plugin source. A pill built from it would name a marketplace
    // the user cannot browse, and picking it would empty the list.
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
    // The Installed-only toggle or a removed marketplace can drop the selection
    // out of the rows. Keeping it would filter the list down to nothing with no
    // pill lit to explain why.
    expect(resolveActiveMarketplace('mkt-gone', available)).toBeNull();
  });

  it('is All with nothing selected', () => {
    expect(resolveActiveMarketplace(null, available)).toBeNull();
  });

  it('keeps a lone surviving marketplace selected, because the dropdown still shows it', () => {
    // The control is drawn whatever the count, so a filter can never be on with
    // nothing on screen saying so. An earlier pill bar hid itself below two
    // marketplaces, which is what made this case dangerous then.
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
    // Their synthetic id matches no real marketplace, which is right: the plugin
    // did not come from the one the user is looking at.
    expect(matchesFilters(ORPHAN, '', 'mkt-a', null)).toBe(false);
  });
});

// A notification deep-link scrolls to one plugin row and pulses it. When the row
// is absent the panel gives the target up, so it cannot linger after the plugin
// is uninstalled. Giving up while a filter is hiding the row instead loses the
// notification's whole point, and the target is consumed for good.
describe('pluginDeepLinkCouldBeFiltered: when a missing row proves nothing', () => {
  it('holds the target while any one filter is on', () => {
    expect(pluginDeepLinkCouldBeFiltered('budget', null, null)).toBe(true);
    expect(pluginDeepLinkCouldBeFiltered('', 'mkt-a', null)).toBe(true);
    expect(pluginDeepLinkCouldBeFiltered('', null, 'finance')).toBe(true);
  });

  it('counts the marketplace filter, which outlives the panel', () => {
    // It is a signal rather than component state, so it can be narrowing the
    // list on the very mount the deep-link arrives at. The search box and the
    // category both reset with the panel, so this is the one that bites.
    expect(pluginDeepLinkCouldBeFiltered('', 'mkt-a', null)).toBe(true);
  });

  it('gives up only with every filter off', () => {
    expect(pluginDeepLinkCouldBeFiltered('', null, null)).toBe(false);
    // Whitespace is not a search: it hides nothing, so it must not hold a
    // target the panel should drop.
    expect(pluginDeepLinkCouldBeFiltered('   ', null, null)).toBe(false);
  });
});
