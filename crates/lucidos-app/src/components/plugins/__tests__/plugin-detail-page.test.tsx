// @vitest-environment jsdom
/**
 * The plugin detail page and the row that opens it (ADR 0414, Phase 4).
 */
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render } from 'preact';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
const uninstallMarketplacePlugin = vi.fn();
vi.mock('../../../store/actions/plugin-uninstall', () => ({ uninstallMarketplacePlugin }));
vi.mock('../../../store/actions/plugin-marketplaces', () => ({ refreshPluginCatalog: vi.fn() }));
vi.mock('../../../store/actions/plugins', async (importOriginal) => ({
  ...await importOriginal<typeof import('../../../store/actions/plugins')>(),
  loadInstalledPlugins: vi.fn(),
}));

const { PluginStoreRow, matchesFilters } = await import('../StoreTab');
const { PluginDetailPage } = await import('../PluginDetailPage');
const { PluginActions, PluginModifiedNotice, PluginTitle } = await import('../pluginCard');
import { renderPluginReadme } from '../pluginReadme';
import { contentViewKey } from '../../layout/contentViewKey';
import { installedPlugins, marketplaceCatalog, panelOverlay } from '../../../store/store';
import type { MarketplacePlugin, PluginMedia } from '../../../store/types';

const source = (rel: string): string => readFileSync(new URL(rel, import.meta.url), 'utf8');

const NO_MEDIA: PluginMedia = { screenshots: [], videos: [], problems: [] };

function row(over: Partial<MarketplacePlugin>): MarketplacePlugin {
  return {
    marketplace_id: 'mkt-a',
    marketplace_name: 'Alpha',
    id: 'habit-tracker',
    name: 'Habit Tracker',
    description: 'Track a habit a day.',
    version: '0.1.0',
    source: 'https://example.com/habit-tracker',
    manifest: {},
    content: ['apps'],
    categories: [],
    files_count: 3,
    engine_compatible: true,
    engine_requirement: '>=0.46.1',
    status: 'available',
    media: NO_MEDIA,
    ...over,
  };
}

let host: HTMLDivElement | null = null;
const initialCatalog = marketplaceCatalog.value;
const initialInstalled = installedPlugins.value;

function show(plugin: MarketplacePlugin, stageInstall: (p: MarketplacePlugin) => void = () => {}): HTMLDivElement {
  host = document.createElement('div');
  document.body.appendChild(host);
  render(<PluginStoreRow plugin={plugin} installingSource={null} stageInstall={stageInstall} />, host);
  return host;
}

const MODIFIED_INSTALL: Partial<MarketplacePlugin> = {
  status: 'installed',
  installed_version: '0.1.0',
  modified: true,
  categories: ['productivity'],
};

afterEach(() => {
  if (host) {
    render(null, host);
    host.remove();
  }
  host = null;
  panelOverlay.value = null;
  marketplaceCatalog.value = initialCatalog;
  installedPlugins.value = initialInstalled;
});

describe.each(['panel', 'detail'])('plugin description in the %s', (surface) => {
  it('renders separate plain-text paragraphs and folds single newlines', () => {
    const plugin = row({ description: ' First\r\n line.\r\n \t\r\n**Second** <img src=x onerror=alert(1)>\n\nLast searchable phrase. ' });
    if (surface === 'panel') {
      show(plugin);
    } else {
      marketplaceCatalog.value = {
        status: 'loaded',
        data: { marketplaces: [], plugins: [plugin], errors: [], scanned_at: null, scanning: false, scan_error: null },
      };
      installedPlugins.value = { status: 'loaded', data: [] };
      host = document.createElement('div');
      document.body.appendChild(host);
      render(<PluginDetailPage marketplaceId={plugin.marketplace_id} pluginId={plugin.id} name={plugin.name} />, host);
    }
    const description = host!.querySelector(`.${surface === 'panel' ? 'app-store-plugin' : 'plugin-detail'}-description`)!;
    expect(Array.from(description.querySelectorAll('p'), (p) => p.textContent)).toEqual([
      'First line.', '**Second** <img src=x onerror=alert(1)>', 'Last searchable phrase.',
    ]);
    expect(description.querySelector('img, strong, script')).toBeNull();
    expect(description.textContent).toContain('line.\n**Second**');
    expect(matchesFilters(plugin, 'last searchable phrase', null, null)).toBe(true);
  });
});

describe('a Plugins panel row', () => {
  it('leads with the plugin icon from the media route', () => {
    const el = show(row({ media: { ...NO_MEDIA, icon_url: '/api/v1/plugins/media/installed/habit-tracker/media/icon.svg' } }));
    const img = el.querySelector<HTMLImageElement>('.app-store-plugin-icon .app-icon-image img');
    expect(img?.getAttribute('src')).toContain('/api/v1/plugins/media/installed/habit-tracker/media/icon.svg');
  });

  it('shows the monogram tile when the plugin has no icon', () => {
    const el = show(row({}));
    expect(el.querySelector('.app-store-plugin-icon .app-icon-monogram')?.textContent).toBe('H');
  });

  it('draws no screenshot, which the detail page holds', () => {
    const el = show(row({ media: { ...NO_MEDIA, screenshots: ['/api/v1/plugins/media/x/a.png'] } }));
    expect(el.querySelectorAll('img')).toHaveLength(0);
  });

  it('opens the plugin detail page on a tap', () => {
    const el = show(row({}));
    el.querySelector<HTMLElement>('.app-store-plugin-row')!.click();
    expect(panelOverlay.value).toEqual({
      type: 'plugin-detail',
      marketplaceId: 'mkt-a',
      pluginId: 'habit-tracker',
      name: 'Habit Tracker',
    });
  });

  it('keeps a button tap to the button', () => {
    const stageInstall = vi.fn();
    const el = show(row({}), stageInstall);
    el.querySelector<HTMLElement>('.list-row-actions .action-btn')!.click();
    expect(stageInstall).toHaveBeenCalledOnce();
    expect(panelOverlay.value).toBeNull();
  });

  it('shows one button and leaves Uninstall, the modified notice and the chips to the detail page', () => {
    const el = show(row({ ...MODIFIED_INSTALL, app_id: 'habit-tracker' }));
    const buttons = Array.from(el.querySelectorAll('.list-row-actions button'), (b) => b.textContent);
    expect(buttons).toEqual(['Open']);
    expect(el.querySelector('.app-store-modified-chip')).toBeNull();
    expect(el.querySelector('.app-store-status')).toBeNull();
    expect(el.querySelector('.app-store-plugin-meta')).toBeNull();
  });
});

describe('the plugin detail page title', () => {
  it('keeps the version and the Modified chip the row leaves out', () => {
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<PluginTitle plugin={row({ ...MODIFIED_INSTALL, app_id: 'habit-tracker' })} nameClass="title" />, host);
    expect(host.querySelector('.app-store-status')?.textContent).toMatch(/^Installed v/);
    expect(host.querySelector('.app-store-modified-chip')?.textContent).toBe('Modified');
  });
});

describe('the plugin detail page buttons', () => {
  it('offers Uninstall beside the primary button', () => {
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<PluginActions plugin={row({ ...MODIFIED_INSTALL, app_id: 'habit-tracker' })} />, host);
    const buttons = Array.from(host.querySelectorAll('.list-row-actions button'), (b) => b.textContent);
    expect(buttons).toEqual(['Uninstall', 'Open']);
    host.querySelector<HTMLElement>('.action-btn-secondary')!.click();
    expect(uninstallMarketplacePlugin).toHaveBeenCalledOnce();
  });
});

describe('the plugin detail page modified notice', () => {
  function showNotice(plugin: MarketplacePlugin): HTMLDivElement {
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<PluginModifiedNotice plugin={plugin} />, host);
    return host;
  }

  it('counts the changed files and offers to send them to the author', () => {
    const el = showNotice(row({ ...MODIFIED_INSTALL, modified_paths: ['a.html', 'b.js', 'c.css'] }));
    expect(el.textContent).toContain('You changed 3 files in this plugin.');
    expect(el.querySelector('button')?.textContent).toBe('Send changes to author');
  });

  it('draws nothing for an unmodified plugin', () => {
    const el = showNotice(row({ status: 'installed', installed_version: '0.1.0' }));
    expect(el.querySelector('.plugin-detail-modified')).toBeNull();
  });
});

describe('the plugin detail page view key', () => {
  it('tells two plugins apart', () => {
    const key = (pluginId: string) => contentViewKey('plugins', {
      type: 'plugin-detail', marketplaceId: 'mkt-a', pluginId, name: pluginId,
    }, 'main');
    expect(key('a')).not.toBe(key('b'));
  });
});

/** Invariant I7: the README cannot inject markup or reach a remote host. */
describe('the plugin README', () => {
  it('drops script, event handlers and remote elements', () => {
    const html = renderPluginReadme([
      '# Hello',
      '<script>window.hacked = 1</script>',
      '<img src="https://tracker.example/p.png" onerror="window.hacked = 1">',
      '<iframe src="https://example.com"></iframe>',
      '![shot](https://tracker.example/shot.png)',
      '<div style="background-image:url(https://tracker.example/s)">styled</div>',
      '<table background="https://tracker.example/t"><tr><td>x</td></tr></table>',
      '<input type="image" src="https://tracker.example/i">',
      'A [link](https://example.com) and a [relative one](docs/setup.md).',
    ].join('\n\n'));
    const doc = new DOMParser().parseFromString(html, 'text/html');
    expect(doc.querySelector('script, img, iframe, input')).toBeNull();
    expect(doc.querySelector('[style], [background], [src]')).toBeNull();
    expect(html).not.toContain('onerror');
    // The authored input shows as text, so only a live attribute counts.
    const attributeValues = Array.from(doc.querySelectorAll('*'))
      .flatMap((el) => Array.from(el.attributes, (attr) => attr.value));
    expect(attributeValues.join(' ')).not.toContain('tracker.example');
    expect(doc.querySelector('h1')?.textContent).toBe('Hello');
    const [outbound, relative] = Array.from(doc.querySelectorAll('a'));
    expect(outbound.getAttribute('href')).toBe('https://example.com');
    expect(outbound.getAttribute('target')).toBe('_blank');
    expect(relative.hasAttribute('href')).toBe(false);
  });
});

/** Invariants I6 and I13: media reaches the DOM as `<img>` and `<video>` only,
 *  and the page mounts no frame, so no plugin code runs before install. */
describe('the plugin detail page source', () => {
  const page = source('../PluginDetailPage.tsx');

  it('mounts no frame and draws no media as markup', () => {
    expect(page).not.toMatch(/<(iframe|object|embed|svg)\b/);
  });

  it('sets inner HTML only for the sanitized README', () => {
    const uses = page.split('\n').filter((line) => line.includes('dangerouslySetInnerHTML'));
    expect(uses).toEqual([expect.stringContaining('__html: html')]);
    expect(page).toContain('renderPluginReadme(readme.data)');
  });
});
