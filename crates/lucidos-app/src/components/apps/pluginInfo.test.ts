import { describe, it, expect } from 'vitest';
import { resolvePluginInfo } from './pluginInfo';
import type { MarketplacePlugin } from '../../store/types';

function plugin(overrides: Partial<MarketplacePlugin>): MarketplacePlugin {
  return {
    marketplace_id: 'mkt',
    marketplace_name: 'Test Marketplace',
    id: 'p',
    name: 'Plugin',
    description: '',
    version: '1.0.0',
    source: 'https://example.com/p',
    manifest: {},
    content: ['apps'],
    categories: [],
    files_count: 1,
    engine_compatible: true,
    status: 'installed',
    app_id: 'p',
    ...overrides,
  };
}

describe('resolvePluginInfo', () => {
  it('labels an installed app with its marketplace', () => {
    const map = resolvePluginInfo([plugin({ app_id: 'weather', marketplace_name: 'Acme' })]);
    expect(map.get('weather')?.marketplaceName).toBe('Acme');
    expect(map.get('weather')?.updateAvailable).toBe(false);
  });

  it('flags update_available', () => {
    const map = resolvePluginInfo([plugin({ app_id: 'weather', status: 'update_available' })]);
    expect(map.get('weather')?.updateAvailable).toBe(true);
  });

  it('skips not-installed (available) plugins and plugins with no app', () => {
    const map = resolvePluginInfo([
      plugin({ app_id: 'avail', status: 'available' }),
      plugin({ app_id: undefined }),
    ]);
    expect(map.size).toBe(0);
  });

  it('does not offer an update this Lucidos cannot install, and says why', () => {
    const map = resolvePluginInfo([
      plugin({
        app_id: 'weather',
        status: 'update_available',
        engine_compatible: false,
        engine_incompatible_reason: 'Needs Lucidos 0.46.1 or later',
      }),
    ]);
    expect(map.get('weather')?.updateAvailable).toBe(false);
    expect(map.get('weather')?.updateBlockedReason).toBe('Needs Lucidos 0.46.1 or later');
  });

  it('prefers an installable update over a blocked one across marketplaces', () => {
    const map = resolvePluginInfo([
      plugin({
        app_id: 'weather',
        marketplace_name: 'A',
        status: 'update_available',
        engine_compatible: false,
        engine_incompatible_reason: 'Needs Lucidos 0.99.0 or later',
      }),
      plugin({ app_id: 'weather', marketplace_name: 'B', status: 'update_available' }),
    ]);
    expect(map.get('weather')?.updateAvailable).toBe(true);
    expect(map.get('weather')?.marketplaceName).toBe('B');
  });

  it('prefers the update_available entry when an app id spans two marketplaces', () => {
    const map = resolvePluginInfo([
      plugin({ app_id: 'weather', marketplace_name: 'A', status: 'installed' }),
      plugin({ app_id: 'weather', marketplace_name: 'B', status: 'update_available' }),
    ]);
    expect(map.get('weather')?.updateAvailable).toBe(true);
    expect(map.get('weather')?.marketplaceName).toBe('B');
  });
});
