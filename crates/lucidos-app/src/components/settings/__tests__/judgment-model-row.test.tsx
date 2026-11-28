// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/preferences', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/preferences')>()),
  savePreference: vi.fn(async () => {}),
  saveModelSelection: vi.fn(async () => {}),
}));

import { JudgmentModelRow } from '../JudgmentModelRow';
import type { BackgroundRows } from '../useBackgroundModels';
import { savePreference, saveModelSelection } from '../../../store/actions/preferences';
import { chatModels, configuredProviders, credentials, preferences } from '../../../store/store';
import type { BackgroundModel, ModelInfo } from '../../../api/types';
import type { CredentialInfo } from '../../../store/types';

function registryRow(id: string, label: string, efforts: string[]): ModelInfo {
  return {
    id,
    label,
    routes: [{ provider: 'vertex', id, reasoning_efforts: efforts }],
    preferred_provider: null,
    vision: false,
    default_effort: null,
    successor: null,
    sort_order: 0,
    source: 'builtin',
    on_chat_ladder: true,
    created_at: '2026-01-01T00:00:00Z',
  };
}

const REGISTRY: ModelInfo[] = [
  registryRow('claude-haiku-4-5-20251001', 'Haiku 4.5', ['none', 'low']),
  registryRow('gemini-3.8-flash', 'Gemini 3.8 Flash', ['low', 'medium', 'high']),
];

const UNRESOLVED: BackgroundRows = { row: () => null, error: null, refresh: async () => {} };

function resolvedTo(row: Partial<BackgroundModel>): BackgroundRows {
  return {
    row: () => ({
      model: 'gemini-3.8-flash',
      effort: null,
      source: 'default',
      reachable: true,
      generation: null,
      not_served: [],
      needs_vision: false,
      asks_no_model: false,
      unresolved: null,
      vision: true,
      default_effort: null,
      recommended: [],
      ...row,
    }),
    error: null,
    refresh: async () => {},
  };
}

function typeSafeCred(): CredentialInfo {
  return {
    id: 'c1',
    service_name: 'typesafe',
    base_urls: ['https://api.typesafe.ai/v1'],
    auth_type: 'api_key',
    auth_header: 'Authorization',
    created_at: '2026-09-19T00:00:00Z',
  } as CredentialInfo;
}

describe('JudgmentModelRow', () => {
  let host: HTMLElement;

  const trigger = () => host.querySelector<HTMLButtonElement>('.dropdown-trigger');
  const triggerLabel = () => host.querySelector('.model-selection-value')?.textContent ?? '';
  const detail = () => host.querySelector('.list-row-details')?.textContent ?? '';
  const options = () => [...document.body.querySelectorAll<HTMLElement>('.control-option')];
  const optionValues = () => options().map((o) => o.dataset.value ?? '');

  function open() {
    act(() => { trigger()!.click(); });
  }

  function pick(value: string) {
    const match = options().find((o) => o.dataset.value === value);
    if (!match) throw new Error(`no option "${value}" in ${optionValues().join(', ')}`);
    act(() => { match.click(); });
  }

  function mount(background: BackgroundRows = UNRESOLVED) {
    act(() => {
      render(
        <JudgmentModelRow
          site="command-guard"
          label="Command judge"
          anchor="models:command-judge"
          background={background}
        />,
        host,
      );
    });
  }

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    vi.mocked(savePreference).mockClear();
    vi.mocked(saveModelSelection).mockClear();
    preferences.value = { status: 'loaded', data: {} };
    credentials.value = { status: 'loaded', data: [] };
    chatModels.value = { status: 'loaded', data: REGISTRY };
    configuredProviders.value = null;
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    preferences.value = { status: 'not-loaded' };
    credentials.value = { status: 'not-loaded' };
    chatModels.value = { status: 'not-loaded' };
  });

  it('offers only the chat models with no TypeSafe key', () => {
    mount();
    open();
    expect(optionValues()).toEqual(['claude-haiku-4-5-20251001', 'gemini-3.8-flash']);
  });

  it('offers no Jev row while the credentials are still loading', () => {
    credentials.value = { status: 'loading' };
    mount();
    open();
    expect(optionValues()).not.toContain('typesafe-jev');
  });

  it('offers the Jev row once a key is stored', () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    mount();
    open();
    expect(optionValues()).toEqual(['claude-haiku-4-5-20251001', 'gemini-3.8-flash', 'typesafe-jev']);
  });

  it('reads the backend in force on the trigger', () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    preferences.value = { status: 'loaded', data: { judgment_command_guard: 'jev' } };
    mount();
    expect(triggerLabel()).toBe('TypeSafe (Jev)');
  });

  it('reads the stored chat pair otherwise', () => {
    preferences.value = {
      status: 'loaded',
      data: { model_command_judge: 'claude-haiku-4-5-20251001', reasoning_command_judge: 'low' },
    };
    mount();
    expect(triggerLabel()).toBe('Haiku 4.5 · Low');
  });

  it('writes only the judgment key when Jev is picked', () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    mount();
    open();
    pick('typesafe-jev');
    expect(savePreference).toHaveBeenCalledWith('judgment_command_guard', 'jev');
    expect(saveModelSelection).not.toHaveBeenCalled();
  });

  it('moves the backend back to chat and writes the pair', async () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    preferences.value = { status: 'loaded', data: { judgment_command_guard: 'jev' } };
    mount();
    open();
    pick('claude-haiku-4-5-20251001');
    pick('claude-haiku-4-5-20251001|low');
    expect(savePreference).toHaveBeenCalledWith('judgment_command_guard', 'chat');
    await Promise.resolve();
    expect(saveModelSelection).toHaveBeenCalledWith(
      'model_command_judge',
      'reasoning_command_judge',
      { model: 'claude-haiku-4-5-20251001', reasoningEffort: 'low', provider: null },
    );
  });

  it('commits a chat model with the tier picked for it', async () => {
    mount();
    open();
    pick('gemini-3.8-flash');
    pick('gemini-3.8-flash|low');
    expect(savePreference).toHaveBeenCalledWith('judgment_command_guard', 'chat');
    await Promise.resolve();
    expect(saveModelSelection).toHaveBeenCalledWith(
      'model_command_judge',
      'reasoning_command_judge',
      { model: 'gemini-3.8-flash', reasoningEffort: 'low', provider: null },
    );
  });

  it('says so while TypeSafe itself is switched off', () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    preferences.value = {
      status: 'loaded',
      data: { judgment_command_guard: 'jev', provider_enabled_typesafe: 'false' },
    };
    mount();
    expect(triggerLabel()).toBe('TypeSafe (Jev)');
    expect(detail()).toContain('Settings → Providers');
  });

  it('keeps the Jev row offered while TypeSafe is switched off', () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    preferences.value = {
      status: 'loaded',
      data: { judgment_command_guard: 'jev', provider_enabled_typesafe: 'false' },
    };
    mount();
    open();
    expect(optionValues()).toContain('typesafe-jev');
  });

  it('offers both Clef rows on a Cloudflare token, and writes the picked id', () => {
    credentials.value = {
      status: 'loaded',
      data: [{
        ...typeSafeCred(),
        service_name: 'cloudflare-workers-ai',
        base_urls: ['https://api.cloudflare.com/client/v4/accounts/abc123/ai'],
      }],
    };
    mount();
    open();
    expect(optionValues()).toEqual([
      'claude-haiku-4-5-20251001',
      'gemini-3.8-flash',
      'cloudflare-clef',
      'cloudflare-clef-flash',
    ]);
    pick('cloudflare-clef-flash');
    expect(savePreference).toHaveBeenCalledWith('judgment_command_guard', 'clef-flash');
    expect(saveModelSelection).not.toHaveBeenCalled();
  });

  it('shows why no judge model resolves', () => {
    const why = 'No configured provider serves a command judge model (gemini-3.1-flash-lite).';
    mount(resolvedTo({ source: 'chat-model', asks_no_model: true, unresolved: why }));
    expect(host.textContent).toContain(why);
  });

  it('reads the resolved model while unset', () => {
    mount(resolvedTo({ model: 'claude-haiku-4-5-20251001', effort: 'none' }));
    expect(triggerLabel()).toBe('Haiku 4.5 · Off');
  });

  it('lists the recommended models first, each section under a heading', () => {
    credentials.value = { status: 'loaded', data: [typeSafeCred()] };
    mount(resolvedTo({ recommended: [{ model: 'gemini-3.8-flash', effort: null }] }));
    open();
    expect(optionValues()).toEqual(['gemini-3.8-flash', 'claude-haiku-4-5-20251001', 'typesafe-jev']);
    const headings = [...document.body.querySelectorAll('.control-section-label')].map((h) => h.textContent);
    expect(headings).toEqual(['Recommended', 'Other models', 'System One']);
  });

  it('says when no configured provider serves the stored pick', () => {
    preferences.value = { status: 'loaded', data: { model_command_judge: 'gemini-3.8-flash' } };
    mount(resolvedTo({ source: 'preference', reachable: false }));
    expect(detail()).toBe('No configured provider serves this model');
  });
});
