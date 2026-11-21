// @vitest-environment jsdom
/**
 * An outdated plugin stands out: its Update button is green, and the list
 * groups every outdated plugin under an "Updates available" section on top.
 * With nothing to update, the list draws no section headers at all.
 */
import { afterEach, describe, it, expect } from 'vitest';
import { render } from 'preact';
import { PluginRowSections, PluginStoreRow, splitOutdated, type PluginSectionId } from '../StoreTab';
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

const OUTDATED = plugin({ id: 'habit-tracker', name: 'Habit Tracker', status: 'update_available', installed_version: '0.1.0' });
const AVAILABLE = plugin({ id: 'demo-director', name: 'Demo Director' });
const INSTALLED = plugin({ id: 'data-analysis', name: 'Data Analysis', status: 'installed', installed_version: '0.2.0' });

let host: HTMLDivElement | null = null;

function mount(node: preact.ComponentChild): HTMLDivElement {
  host = document.createElement('div');
  render(node, host);
  return host;
}

function sections(plugins: MarketplacePlugin[], installedOnly = false, collapsed: ReadonlySet<PluginSectionId> = new Set()) {
  return mount(
    <PluginRowSections
      plugins={plugins}
      installedOnly={installedOnly}
      collapsed={collapsed}
      onToggle={() => {}}
      installingSource={null}
      stageInstall={() => {}}
    />,
  );
}

const headers = (el: HTMLElement) => [...el.querySelectorAll('.section-label')].map((h) => h.textContent);
const rowIds = (el: HTMLElement) => [...el.querySelectorAll('[data-plugin-id]')].map((r) => r.getAttribute('data-plugin-id'));

afterEach(() => {
  if (host) render(null, host);
  host = null;
});

describe('an outdated plugin', () => {
  it('draws its Update button green', () => {
    const el = mount(<PluginStoreRow plugin={OUTDATED} installingSource={null} stageInstall={() => {}} />);
    const button = el.querySelector('.list-row-actions .action-btn');
    expect(button?.textContent).toBe('Update');
    expect(button?.classList.contains('action-btn-confirm')).toBe(true);
  });

  it('leaves Install in the neutral blue', () => {
    const el = mount(<PluginStoreRow plugin={AVAILABLE} installingSource={null} stageInstall={() => {}} />);
    expect(el.querySelector('.list-row-actions .action-btn')?.classList.contains('action-btn-confirm')).toBe(false);
  });
});

describe('splitOutdated', () => {
  it('keeps catalog order inside each group', () => {
    const second = { ...OUTDATED, id: 'ticket-workflow' };
    expect(splitOutdated([AVAILABLE, OUTDATED, INSTALLED, second])).toEqual({
      updates: [OUTDATED, second],
      rest: [AVAILABLE, INSTALLED],
    });
  });
});

describe('the plugin list sections', () => {
  it('draws no headers when nothing needs an update', () => {
    const el = sections([AVAILABLE, INSTALLED]);
    expect(headers(el)).toEqual([]);
    expect(rowIds(el)).toEqual(['demo-director', 'data-analysis']);
  });

  it('puts outdated plugins first, under their own section', () => {
    const el = sections([AVAILABLE, OUTDATED, INSTALLED]);
    expect(headers(el)).toEqual(['Updates available', 'All plugins']);
    expect(rowIds(el)).toEqual(['habit-tracker', 'demo-director', 'data-analysis']);
  });

  it('names the rest "Up to date" under the Installed toggle', () => {
    expect(headers(sections([OUTDATED, INSTALLED], true))).toEqual(['Updates available', 'Up to date']);
  });

  it('drops the second header when every plugin is outdated', () => {
    expect(headers(sections([OUTDATED]))).toEqual(['Updates available']);
  });

  it('hides a collapsed section\'s rows', () => {
    const el = sections([AVAILABLE, OUTDATED], false, new Set(['updates']));
    expect(rowIds(el)).toEqual(['demo-director']);
  });
});
