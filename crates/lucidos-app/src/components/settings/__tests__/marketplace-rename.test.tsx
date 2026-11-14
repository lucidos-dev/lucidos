// @vitest-environment jsdom
/**
 * Settings → Marketplaces renames a row in place, and leaves its URL alone.
 *
 * The URL is the marketplace's identity: the engine keys the registry entry on
 * a hash of it, so a rename has to re-post the stored URL verbatim. Post a
 * different one and the upsert lands on a new id, leaving the old row behind.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

const SOURCE = 'https://github.com/example-org/example-repo';
const marketplace = (name: string) => ({ id: 'example-repo-1a2b3c4d', name, source: SOURCE });
const catalogOf = (name: string) => ({
  marketplaces: [marketplace(name)],
  plugins: [],
  errors: [],
  scanned_at: '2026-09-22T10:00:00Z',
  scanning: false,
  scan_error: null,
});

const mockFetchPluginCatalog = vi.fn(async () => catalogOf('Example plugins'));
const mockAddPluginMarketplace = vi.fn(async (_source: string, name?: string) => ({
  marketplace: marketplace(name ?? 'Example plugins'),
  marketplaces: [marketplace(name ?? 'Example plugins')],
  created: false,
  commit: 'abc',
}));
const mockRemovePluginMarketplace = vi.fn();
vi.mock('../../../api/client', () => ({
  fetchPluginCatalog: (...args: unknown[]) => mockFetchPluginCatalog(...(args as [])),
  addPluginMarketplace: (...args: unknown[]) =>
    mockAddPluginMarketplace(...(args as [string, string?])),
  removePluginMarketplace: (...args: unknown[]) => mockRemovePluginMarketplace(...args),
  isTransportError: () => false,
}));

import { MarketplacesSection } from '../MarketplacesSection';
import { marketplaceCatalog } from '../../../store/store';

let host: HTMLElement;

function row(): HTMLElement {
  const el = host.querySelector<HTMLElement>('.app-store-marketplace-row');
  if (!el) throw new Error('the marketplace row is not rendered');
  return el;
}

function nameField(): HTMLInputElement {
  const el = row().querySelector<HTMLInputElement>('.app-store-marketplace-name-input');
  if (!el) throw new Error('the rename field is not rendered');
  return el;
}

function renameButton(): HTMLButtonElement {
  const el = row().querySelector<HTMLButtonElement>('.app-store-marketplace-rename');
  if (!el) throw new Error('the rename button is not rendered');
  return el;
}

function typeInto(el: HTMLInputElement, value: string): void {
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

function press(el: HTMLElement, key: string): void {
  el.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
}

/** Let Preact flush. A real user types into a rendered field, so each step here
 *  waits for the render the step before it asked for. */
async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

async function waitFor(done: () => boolean, budgetMs = 1000): Promise<void> {
  for (let waited = 0; waited < budgetMs; waited += 10) {
    if (done()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

describe('renaming a marketplace', () => {
  beforeEach(() => {
    mockAddPluginMarketplace.mockClear();
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    marketplaceCatalog.value = { status: 'loaded', data: catalogOf('Example plugins') };
    render(<MarketplacesSection />, host);
  });

  afterEach(() => {
    render(null, host);
    document.body.innerHTML = '';
    marketplaceCatalog.value = { status: 'not-loaded' };
  });

  it('re-posts the URL the row already holds, so the entry keeps its id', async () => {
    renameButton().click();
    await settle();
    typeInto(nameField(), 'Example marketplace');
    await settle();
    press(nameField(), 'Enter');

    await waitFor(() => mockAddPluginMarketplace.mock.calls.length > 0);

    expect(mockAddPluginMarketplace).toHaveBeenCalledWith(SOURCE, 'Example marketplace');
  });

  it('offers no field for the URL, which is read-only text', () => {
    expect(row().querySelectorAll('input')).toHaveLength(1);
    expect(row().querySelector('code.app-store-source-value')?.textContent).toBe(SOURCE);
  });

  it('keeps the served name when the edit is abandoned with Escape', async () => {
    renameButton().click();
    await settle();
    typeInto(nameField(), 'Half-typed');
    await settle();
    press(nameField(), 'Escape');

    await waitFor(() => nameField().value === 'Example plugins');

    expect(nameField().value).toBe('Example plugins');
    expect(mockAddPluginMarketplace).not.toHaveBeenCalled();
  });

  // ADR 0118: an idle field holds no copy, so a rename from another device
  // repaints it. Seeded once at mount it would offer the stale name back.
  it('follows a frame that renames the marketplace elsewhere', async () => {
    marketplaceCatalog.value = { status: 'loaded', data: catalogOf('Renamed elsewhere') };

    await waitFor(() => nameField().value === 'Renamed elsewhere');

    expect(nameField().value).toBe('Renamed elsewhere');
    expect(row().querySelector('.list-row-name')?.textContent).toBe('Renamed elsewhere');
  });

  it('keeps a name the user is typing when such a frame lands', async () => {
    renameButton().click();
    await settle();
    typeInto(nameField(), 'My own name');
    await settle();
    marketplaceCatalog.value = { status: 'loaded', data: catalogOf('Renamed elsewhere') };

    await new Promise((resolve) => setTimeout(resolve, 60));

    expect(nameField().value).toBe('My own name');
  });
});
