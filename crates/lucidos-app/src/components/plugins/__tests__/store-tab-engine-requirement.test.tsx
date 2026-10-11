// @vitest-environment jsdom
import { afterEach, describe, it, expect } from 'vitest';
import { render } from 'preact';
import { pluginRowsInScope, PluginStoreRow } from '../StoreTab';
import { cardPrimaryAction } from '../pluginCard';
import type { InstalledPlugin, MarketplacePlugin } from '../../../store/types';

function row(over: Partial<MarketplacePlugin>): MarketplacePlugin {
  return {
    marketplace_id: 'mkt-a',
    marketplace_name: 'Alpha',
    id: 'theme-studio',
    name: 'Theme Studio',
    description: '',
    version: '0.1.0',
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

const NEEDS_NEWER = {
  engine_requirement: '>=0.46.1',
  engine_compatible: false,
  engine_incompatible_reason: 'Needs Lucidos 0.46.1 or later',
};

let host: HTMLDivElement | null = null;

function show(plugin: MarketplacePlugin): HTMLDivElement {
  host = document.createElement('div');
  render(<PluginStoreRow plugin={plugin} installingSource={null} stageInstall={() => {}} />, host);
  return host;
}

afterEach(() => {
  if (host) render(null, host);
  host = null;
});

describe('a plugin this Lucidos cannot install', () => {
  it('still lists, with Install disabled and the reason beside it', () => {
    const el = show(row(NEEDS_NEWER));
    const button = el.querySelector<HTMLButtonElement>('.list-row-actions .action-btn:not(.action-btn-secondary)');
    expect(button?.textContent).toBe('Install');
    expect(button?.disabled).toBe(true);
    expect(el.querySelector('[data-role="engine-requirement"]')?.textContent).toBe(
      'Needs Lucidos 0.46.1 or later',
    );
  });

  it('disables Update the same way for an installed plugin with a newer version', () => {
    const action = cardPrimaryAction(
      row({ ...NEEDS_NEWER, status: 'update_available', installed_version: '0.0.9' }),
    );
    expect(action).toEqual({
      kind: 'install',
      label: 'Update',
      blockedReason: 'Needs Lucidos 0.46.1 or later',
    });
  });

  it('never blocks an installed plugin: it keeps its Open action', () => {
    const action = cardPrimaryAction(row({ ...NEEDS_NEWER, status: 'installed', app_id: 'theme-studio' }));
    expect(action).toEqual({ kind: 'open', appId: 'theme-studio' });
  });

  it('offers a live Install when the engine meets the requirement', () => {
    const el = show(row({ engine_requirement: '>=0.46.1' }));
    const button = el.querySelector<HTMLButtonElement>('.list-row-actions .action-btn');
    expect(button?.disabled).toBe(false);
    expect(el.querySelector('[data-role="engine-requirement"]')).toBeNull();
    expect(el.querySelector('[data-role="engine-undeclared"]')).toBeNull();
  });

  it('keeps only its own chip, never the no-requirement one', () => {
    const el = show(row(NEEDS_NEWER));
    expect(el.querySelector('[data-role="engine-undeclared"]')).toBeNull();
  });

  it('reads an empty requirement as declared, since the engine refuses it', () => {
    const el = show(
      row({
        engine_requirement: '',
        engine_compatible: false,
        engine_incompatible_reason: 'Its engine requirement "" is not valid',
      }),
    );
    expect(el.querySelector('[data-role="engine-requirement"]')).not.toBeNull();
    expect(el.querySelector('[data-role="engine-undeclared"]')).toBeNull();
  });
});

describe('a plugin that declares no engine requirement', () => {
  it('shows a muted chip with the sentence as its tooltip, and a live Install', () => {
    const el = show(row({}));
    const chip = el.querySelector<HTMLElement>('[data-role="engine-undeclared"]');
    expect(chip?.textContent).toBe('No version requirement');
    expect(chip?.className).toBe('label label-neutral');
    expect(chip?.dataset.tooltip).toBe("This plugin doesn't say which Lucidos version it needs.");
    const button = el.querySelector<HTMLButtonElement>('.list-row-actions .action-btn');
    expect(button?.textContent).toBe('Install');
    expect(button?.disabled).toBe(false);
    expect(el.querySelector('[data-role="engine-requirement"]')).toBeNull();
  });

  it('keeps Update live for an installed plugin with a newer version', () => {
    const el = show(row({ status: 'update_available', installed_version: '0.0.9' }));
    expect(el.querySelector('[data-role="engine-undeclared"]')).not.toBeNull();
    const button = el.querySelector<HTMLButtonElement>(
      '.list-row-actions .action-btn:not(.action-btn-secondary)',
    );
    expect(button?.textContent).toBe('Update');
    expect(button?.disabled).toBe(false);
  });

  it('reads an orphan row from the installed summary', () => {
    const installed = (engine_requirement?: string): InstalledPlugin => ({
      id: 'trigger-timeline',
      name: 'Trigger Timeline',
      version: '0.2.0',
      content: ['apps'],
      files: ['apps/trigger-timeline/index.html'],
      engine_requirement,
      media: { screenshots: [], videos: [], problems: [] },
    });
    const orphan = (p: InstalledPlugin) =>
      pluginRowsInScope({ status: 'loaded', data: { plugins: [] } }, { status: 'loaded', data: [p] }, true)[0];

    const undeclared = show(orphan(installed()));
    expect(undeclared.querySelector('[data-role="engine-undeclared"]')).not.toBeNull();
    render(null, undeclared);

    const declared = show(orphan(installed('>=0.30.0')));
    expect(declared.querySelector('[data-role="engine-undeclared"]')).toBeNull();
  });
});
