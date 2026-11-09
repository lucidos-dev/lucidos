// @vitest-environment jsdom
/**
 * Settings → Providers owns every provider block, and Settings → Models owns
 * none. Search lands on the page, and an anchor retired with the move still
 * reaches its block.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { ProvidersPage } from '../ProvidersPage';
import { SYSTEM_ONE_PROVIDERS } from '../judgmentBackend';
import { PROVIDERS, providerAnchor } from '../../../store/modelProviders';
import { aliasRetiredSettingsAnchor, credentials, preferences, SETTINGS_NAV_ITEMS } from '../../../store/store';
import { findSettingsEntry, settingsSearchEntryIds } from '../../search/searchIndex';

const SETTINGS_VIEW = readFileSync(
  resolve(dirname(fileURLToPath(import.meta.url)), '..', 'SettingsView.tsx'),
  'utf8',
) as string;

const MODEL_PROVIDER_ANCHORS = PROVIDERS.map((p) => providerAnchor(p.value));
const SYSTEM_ONE_ANCHORS = Object.values(SYSTEM_ONE_PROVIDERS).map((p) => p.anchor);

describe('ProvidersPage', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    preferences.value = { status: 'loaded', data: {} };
    credentials.value = { status: 'loaded', data: [] };
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    preferences.value = { status: 'not-loaded' };
    credentials.value = { status: 'not-loaded' };
  });

  it('draws every model provider and System One block exactly once', async () => {
    await act(() => {
      render(<ProvidersPage />, host);
    });
    const drawn = [...host.querySelectorAll('[data-search-anchor]')]
      .map((el) => el.getAttribute('data-search-anchor'));
    for (const anchor of [...MODEL_PROVIDER_ANCHORS, ...SYSTEM_ONE_ANCHORS]) {
      expect(drawn.filter((a) => a === anchor), anchor).toHaveLength(1);
    }
  });

  it('gives the two sections their own titles', async () => {
    await act(() => {
      render(<ProvidersPage />, host);
    });
    const titles = [...host.querySelectorAll('.settings-section-title')].map((el) => el.textContent);
    expect(titles).toEqual(['Model providers', 'System One']);
  });
});

describe('Settings → Models', () => {
  it('draws no provider block', () => {
    const body = SETTINGS_VIEW.slice(
      SETTINGS_VIEW.indexOf('function modelsSection()'),
      SETTINGS_VIEW.indexOf('function appearanceSection()'),
    );
    expect(body).not.toMatch(/ProviderSettings|JudgmentSettings|CloudflareWorkersAiSettings|CustomSystemOneSettings|ClaudeGatewaySettings|OpenCodeFreeSettings/);
    expect(SETTINGS_VIEW).not.toContain('VertexProviderSettings');
  });
});

describe('the Providers nav entry', () => {
  it('leads the Assistant group', () => {
    const assistant = SETTINGS_NAV_ITEMS.filter((i) => i.group === 'Assistant').map((i) => i.key);
    expect(assistant.slice(0, 3)).toEqual(['providers', 'models', 'intelligence']);
  });
});

describe('provider search entries', () => {
  it('land on the Providers page, on an anchor the page draws', () => {
    const anchors = new Set([...MODEL_PROVIDER_ANCHORS, ...SYSTEM_ONE_ANCHORS]);
    const providerEntries = settingsSearchEntryIds()
      .map((id) => findSettingsEntry(id)!)
      .filter((e) => e.subview === 'providers' && e.anchor);
    expect(providerEntries.length).toBeGreaterThan(0);
    for (const entry of providerEntries) {
      expect(anchors.has(entry.anchor!), `${entry.id} lands on ${entry.anchor}`).toBe(true);
    }
  });

  it('are found by the id a persisted recent still holds', () => {
    const retired: Array<[string, string]> = [
      ['models:providers', 'providers'],
      ['models:vertex-ai', 'providers:vertex'],
      ['models:region', 'providers:vertex-region'],
      ['models:xai', 'providers:xai'],
      ['models:opencode-free', 'providers:opencode-free'],
      ['models:typesafe', 'providers:typesafe'],
      ['models:typesafe-usage', 'providers:typesafe-usage'],
      ['models:openai-usage', 'providers:openai-usage'],
      ['models:cloudflare-workers-ai', 'providers:cloudflare-workers-ai'],
      ['models:system-one-custom-usage', 'providers:system-one-custom-usage'],
    ];
    for (const [old, current] of retired) {
      expect(aliasRetiredSettingsAnchor(old), old).toBe(current);
      expect(findSettingsEntry(old)?.id, old).toBe(current);
      expect(findSettingsEntry(old)?.subview, old).toBe('providers');
    }
  });

  it('leaves an anchor that did not move alone', () => {
    expect(aliasRetiredSettingsAnchor('models:manage')).toBe('models:manage');
    expect(aliasRetiredSettingsAnchor('constructor')).toBe('constructor');
  });
});
