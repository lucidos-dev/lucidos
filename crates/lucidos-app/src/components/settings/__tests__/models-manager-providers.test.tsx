// @vitest-environment jsdom
/**
 * Settings → Models points at Settings → Providers three ways. A row counts the
 * providers that are set up. A link chip sits on each model no configured
 * provider serves. The Add Model form offers only providers that are set up.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ModelsManager } from '../ModelsManager';
import {
  chatModels, configuredProviders, settingsScrollTarget, settingsSubview,
} from '../../../store/store';
import { PROVIDERS } from '../../../store/models';
import type { ModelInfo } from '../../../api/types';

let host: HTMLDivElement;

function model(id: string, providers: string[], over: Partial<ModelInfo> = {}): ModelInfo {
  return {
    id,
    label: id,
    routes: providers.map((provider) => ({ provider, id, reasoning_efforts: [] })),
    preferred_provider: null,
    vision: false,
    default_effort: null,
    successor: null,
    sort_order: 0,
    source: 'builtin',
    on_chat_ladder: true,
    created_at: '2026-01-01T00:00:00Z',
    ...over,
  };
}

const SERVABLE = model('servable-model', ['vertex']);
const UNSERVABLE = model('unservable-model', ['openai', 'openrouter']);
const RETIRED = model('retired-model', ['openai'], { successor: 'servable-model' });

function mount(configured: string[] | null) {
  configuredProviders.value = configured;
  chatModels.value = { status: 'loaded', data: [SERVABLE, UNSERVABLE, RETIRED] };
  act(() => { render(<ModelsManager />, host); });
}

const toggle = (label: string) =>
  [...host.querySelectorAll<HTMLButtonElement>('.settings-disclosure-toggle')]
    .find((b) => b.textContent?.includes(label));

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  settingsSubview.value = 'models';
  settingsScrollTarget.value = null;
});

afterEach(() => {
  render(null, host);
  host.remove();
  chatModels.value = { status: 'not-loaded' };
  configuredProviders.value = null;
});

describe('the Providers link row', () => {
  const rowText = () => host.querySelector('.settings-row')?.textContent ?? '';

  it('counts the model providers that are set up', () => {
    mount(['vertex', 'xai', 'typesafe']);
    expect(rowText()).toContain(`Providers, 2 of ${PROVIDERS.length} set up`);
  });

  it('shows no count while the engine has not said', () => {
    mount(null);
    expect(rowText()).toContain('Providers');
    expect(rowText()).not.toContain('set up');
  });

  it('opens the Providers page', () => {
    mount(['vertex']);
    act(() => { host.querySelector<HTMLButtonElement>('.settings-row .action-btn')!.click(); });
    expect(settingsSubview.value).toBe('providers');
  });
});

describe('the Needs a provider group', () => {
  it('is closed, and counts its models', () => {
    mount(['vertex']);
    const button = toggle('Needs a provider (1)');
    expect(button?.getAttribute('aria-expanded')).toBe('false');
  });

  it('is absent when every provider counts as configured', () => {
    mount(null);
    expect(toggle('Needs a provider')).toBeUndefined();
  });

  it('keeps a retired model under Retired, whatever its providers', () => {
    mount(['vertex']);
    expect(toggle('Retired (1)')).toBeDefined();
    expect(toggle('Needs a provider (1)')).toBeDefined();
  });

  it('links each row to its providers in route order', () => {
    mount(['vertex']);
    act(() => { toggle('Needs a provider')!.click(); });
    const chips = [...host.querySelectorAll<HTMLButtonElement>('.model-provider-setup')];
    expect(chips.map((c) => c.textContent)).toEqual(['openai ›', 'openrouter ›']);
    act(() => { chips[1]!.click(); });
    expect(settingsSubview.value).toBe('providers');
    expect(settingsScrollTarget.value).toBe('providers:openrouter');
  });
});

describe('the Add Model form', () => {
  const open = () => {
    act(() => {
      [...host.querySelectorAll<HTMLButtonElement>('button')]
        .find((b) => b.textContent?.includes('Add Model'))!.click();
    });
  };
  const addButton = () =>
    [...host.querySelectorAll<HTMLButtonElement>('.action-btn-confirm')]
      .find((b) => b.textContent === 'Add')!;

  it('offers the first set-up provider, and says the list is filtered', () => {
    mount(['xai', 'vertex']);
    open();
    expect(host.textContent).toContain('Lists the providers you have set up.');
    expect(host.textContent).toContain('Vertex');
    expect(host.textContent).not.toContain('Anthropic (direct)');
  });

  it('disables Add and links to Providers when none is set up', () => {
    mount([]);
    open();
    expect(addButton().disabled).toBe(true);
    const link = [...host.querySelectorAll<HTMLButtonElement>('.accent-link')]
      .find((b) => b.textContent?.includes('Settings → Providers'))!;
    act(() => { link.click(); });
    expect(settingsSubview.value).toBe('providers');
  });
});
