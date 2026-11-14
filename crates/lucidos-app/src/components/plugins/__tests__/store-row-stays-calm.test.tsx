// @vitest-environment jsdom
/**
 * A Store row shows the name, the description and one button. The version
 * and the Modified chip live on the plugin detail page.
 */
import { afterEach, describe, it, expect } from 'vitest';
import { render } from 'preact';
import { PluginStoreRow } from '../StoreTab';
import type { MarketplacePlugin } from '../../../store/types';

function plugin(over: Partial<MarketplacePlugin>): MarketplacePlugin {
  return {
    marketplace_id: 'mkt-a',
    marketplace_name: 'Alpha',
    id: 'theme-studio',
    name: 'Theme Studio',
    description: 'Design themes',
    version: '0.2.0',
    source: 'https://example.com/theme-studio',
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

let host: HTMLDivElement | null = null;

function show(p: MarketplacePlugin): HTMLDivElement {
  host = document.createElement('div');
  render(<PluginStoreRow plugin={p} installingSource={null} stageInstall={() => {}} />, host);
  return host;
}

afterEach(() => {
  if (host) render(null, host);
  host = null;
});

describe('a Store row', () => {
  it('names no version, even with an update waiting', () => {
    const el = show(plugin({ status: 'update_available', installed_version: '0.1.0', modified: true }));
    expect(el.textContent).not.toMatch(/v0\./);
    expect(el.querySelector('.app-store-status')).toBeNull();
    expect(el.querySelector('.app-store-modified-chip')).toBeNull();
    expect(el.querySelector('.list-row-actions .action-btn')?.textContent).toBe('Update');
  });

  it('says Installed where an installed plugin has nothing to open', () => {
    const el = show(plugin({ status: 'installed', installed_version: '0.2.0' }));
    expect(el.querySelector('.list-row-actions button')).toBeNull();
    expect(el.querySelector('.list-row-actions')?.textContent).toBe('Installed');
  });
});
