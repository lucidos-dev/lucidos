/**
 * The marketplace filter is a dropdown in the Plugins filter bar, and its last
 * row adds a marketplace.
 *
 * Two things ride on that last row. Adding a marketplace used to be reachable
 * from this panel only while it was EMPTY: the catalog empty state offered the
 * official-marketplace button and a "register your own marketplace" link.
 * Register one and both leave with the empty state, so the second marketplace
 * had to be found under Settings. And the filter itself is worth nothing until
 * a second one exists, so the two belong in one control.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { Dropdown } from '../../shared/Dropdown';
import { pluginsMarketplaceFilter } from '../../../store/store';

const { openSettingsSubviewMock } = vi.hoisted(() => ({ openSettingsSubviewMock: vi.fn() }));
vi.mock('../../../store/actions/menu', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/menu')>()),
  openSettingsSubview: openSettingsSubviewMock,
}));

const { PluginsView, marketplaceDropdownOptions } = await import('../PluginsView');

const TWO = [
  { id: 'mkt-a', name: 'Alpha' },
  { id: 'mkt-z', name: 'Zulu' },
];

/** Find the Dropdown vnode in PluginsView's tree, without invoking any nested
 *  function component (their hooks would throw outside a real render). */
function findDropdown(node: ComponentChildren): VNode<Record<string, unknown>> | undefined {
  if (node === null || node === undefined || typeof node === 'boolean') return undefined;
  if (typeof node === 'string' || typeof node === 'number') return undefined;
  if (Array.isArray(node)) {
    for (const n of node) {
      const hit = findDropdown(n);
      if (hit) return hit;
    }
    return undefined;
  }
  const v = node as VNode<Record<string, unknown>>;
  // Compared as unknown: the walker's node type is the generic vnode type, which
  // TS sees as having no overlap with Dropdown's own props signature.
  if ((v.type as unknown) === (Dropdown as unknown)) return v;
  if (typeof v.type === 'function') return undefined; // do NOT invoke
  return findDropdown(v.props?.children as ComponentChildren);
}

describe('marketplaceDropdownOptions: what the control offers', () => {
  it('puts All first and the Add shortcut last, marketplaces in between', () => {
    expect(marketplaceDropdownOptions(TWO).map((o) => o.label)).toEqual([
      'All marketplaces',
      'Alpha',
      'Zulu',
      'Add marketplace…',
    ]);
  });

  it('still offers Add with one marketplace, and with none', () => {
    // The redundant filter is the price of keeping the only always-present way
    // to register a marketplace on screen.
    expect(marketplaceDropdownOptions([{ id: 'mkt-a', name: 'Alpha' }]).map((o) => o.label)).toEqual(
      ['All marketplaces', 'Alpha', 'Add marketplace…'],
    );
    expect(marketplaceDropdownOptions([]).map((o) => o.label)).toEqual([
      'All marketplaces',
      'Add marketplace…',
    ]);
  });

  it('gives the two non-marketplace rows values a real id cannot take', () => {
    // The engine builds a marketplace id out of ASCII alphanumerics and dashes,
    // so a colon is what keeps a sentinel from colliding with one.
    const sentinels = marketplaceDropdownOptions([])
      .map((o) => o.value)
      .filter((v) => v.includes(':'));
    expect(sentinels).toHaveLength(2);
    expect(marketplaceDropdownOptions(TWO).map((o) => o.value)).toContain('mkt-a');
  });
});

describe('the dropdown in the Plugins filter bar', () => {
  const ADD = marketplaceDropdownOptions([]).slice(-1)[0].value;

  function onChange(): (v: string) => void {
    return findDropdown(PluginsView())?.props?.onChange as (v: string) => void;
  }

  beforeEach(() => {
    openSettingsSubviewMock.mockClear();
    pluginsMarketplaceFilter.value = null;
  });

  it('is rendered, and sits in the bar via its own right-aligning class', () => {
    const dropdown = findDropdown(PluginsView());
    expect(dropdown, 'no <Dropdown> in PluginsView').toBeTruthy();
    expect(dropdown?.props?.class).toBe('plugins-marketplace-filter');
  });

  it('routes the Add row to Settings and leaves the filter alone', () => {
    pluginsMarketplaceFilter.value = 'mkt-a';
    onChange()(ADD);
    expect(openSettingsSubviewMock).toHaveBeenCalledWith('marketplaces');
    // Add navigates, it does not select. A filter replaced by a trip to
    // Settings would silently widen the list the user was looking at.
    expect(pluginsMarketplaceFilter.value).toBe('mkt-a');
  });

  it('picking All clears the filter rather than storing a sentinel', () => {
    pluginsMarketplaceFilter.value = 'mkt-a';
    onChange()('filter:all');
    expect(pluginsMarketplaceFilter.value).toBeNull();
    expect(openSettingsSubviewMock).not.toHaveBeenCalled();
  });

  it('picking a marketplace stores its id', () => {
    onChange()('mkt-z');
    expect(pluginsMarketplaceFilter.value).toBe('mkt-z');
    expect(openSettingsSubviewMock).not.toHaveBeenCalled();
  });
});
