import { describe, it, expect, vi, beforeEach } from 'vitest';

const setPreferenceMock = vi.fn(async (..._args: unknown[]) => ({ ok: true }));
const getPreferencesMock = vi.fn(async (..._args: unknown[]) => ({
  preferences: {} as Record<string, string>,
}));
const listModelsMock = vi.fn(async () => ({ models: [] as ModelInfo[] }));

// Spreads the real module first: `loadPreferences` also calls
// `retryTransientRead`, and a mock that omitted it would silently throw
// inside the try/catch, masking every assertion below as "stayed at default".
vi.mock('../../../api/client', async (importActual) => ({
  ...(await importActual<typeof import('../../../api/client')>()),
  setPreference: (key: string, value: string, deviceId?: string) =>
    setPreferenceMock(key, value, deviceId),
  getPreferences: (deviceId?: string) => getPreferencesMock(deviceId),
  listModels: () => listModelsMock(),
}));

import { currentModel, preferences, chatModels } from '../../../store/store';
import { accountChatEffort, setChatModelSelection, loadPreferences } from '../../../store/actions/preferences';
import { loadChatModels } from '../../../store/actions/models';
import { DEFAULT_CHAT_MODEL } from '../../../store/models';
import type { ModelInfo } from '../../../api/types';

/** A `provider = local` registry row as the engine serves it: the tiers come
 *  from `llm::reasoning::supported_efforts`, which stops local servers at
 *  `high` because `xhigh` is OpenAI-proprietary. */
const LOCAL_MODEL: ModelInfo = {
  id: 'muse-glimmer:30b-mlx',
  label: 'Muse Glimmer 30B (local)',
  routes: [
    {
      provider: 'local',
      id: 'muse-glimmer:30b-mlx',
      context_window: 131072,
      reasoning_efforts: ['none', 'low', 'medium', 'high'],
    },
  ],
  preferred_provider: null,
  vision: false,
  default_effort: null,
  sort_order: 1000,
  source: 'user',
  enabled: true,
  created_at: '2026-01-01T00:00:00Z',
};

/** A registry row whose provider documents a default effort. */
const OPUS_55: ModelInfo = {
  ...LOCAL_MODEL,
  id: 'claude-opus-5-5',
  label: 'Opus 5.5',
  routes: [{ provider: 'vertex', id: 'claude-opus-5-5', reasoning_efforts: ['low', 'medium', 'high', 'xhigh', 'max'] }],
  default_effort: 'medium',
  source: 'builtin',
};

describe('Chat model and its tier persist across restarts', () => {
  beforeEach(() => {
    setPreferenceMock.mockClear();
    getPreferencesMock.mockClear();
    listModelsMock.mockClear();
    preferences.value = { status: 'loaded', data: {} };
    chatModels.value = { status: 'not-loaded' };
    currentModel.value = DEFAULT_CHAT_MODEL;
  });

  it('setChatModelSelection writes the model preference to the API', async () => {
    await setChatModelSelection({ model: 'claude-sonnet-4-6', reasoningEffort: 'high' });
    expect(currentModel.value).toBe('claude-sonnet-4-6');
    expect(setPreferenceMock).toHaveBeenCalledWith('chat_model', 'claude-sonnet-4-6', undefined);
  });

  it('loadPreferences restores the saved model and its own tier (simulated restart)', async () => {
    getPreferencesMock.mockResolvedValueOnce({
      preferences: {
        chat_model: 'gemini-3.1-pro-preview',
        chat_reasoning_efforts: 'gemini-3.1-pro-preview=medium, claude-sonnet-5=max',
      },
    });
    preferences.value = { status: 'not-loaded' };

    await loadPreferences();

    expect(currentModel.value).toBe('gemini-3.1-pro-preview');
    expect(accountChatEffort('gemini-3.1-pro-preview')).toBe('medium');
    expect(accountChatEffort('claude-sonnet-5')).toBe('max');
  });

  it('loadPreferences honors any stored model value (registry is user-extensible)', async () => {
    getPreferencesMock.mockResolvedValueOnce({ preferences: { chat_model: 'my-custom-model' } });
    preferences.value = { status: 'not-loaded' };

    await loadPreferences();

    expect(currentModel.value).toBe('my-custom-model');
  });

  it('loadPreferences falls back to the default model when none is stored', async () => {
    getPreferencesMock.mockResolvedValueOnce({ preferences: {} });
    currentModel.value = 'something-else';
    preferences.value = { status: 'not-loaded' };

    await loadPreferences();

    expect(currentModel.value).toBe(DEFAULT_CHAT_MODEL);
  });
});

describe('A model with no stored tier runs at its default effort', () => {
  beforeEach(() => {
    preferences.value = { status: 'loaded', data: {} };
    chatModels.value = { status: 'loaded', data: [OPUS_55, LOCAL_MODEL] };
  });

  it('reads the default effort from the registry row', () => {
    expect(accountChatEffort('claude-opus-5-5')).toBe('medium');
  });

  it('a stored tier wins, for that model only', () => {
    preferences.value = { status: 'loaded', data: { chat_reasoning_efforts: 'claude-opus-5-5=xhigh' } };
    expect(accountChatEffort('claude-opus-5-5')).toBe('xhigh');
    expect(accountChatEffort('muse-glimmer:30b-mlx')).toBeNull();
  });

  it('a model with no default effort has no tier, so the engine sends none', () => {
    expect(accountChatEffort('muse-glimmer:30b-mlx')).toBeNull();
  });

  it('a stored tier the route cannot run shows as the tier it snaps to', () => {
    preferences.value = { status: 'loaded', data: { chat_reasoning_efforts: 'muse-glimmer:30b-mlx=max' } };
    expect(accountChatEffort('muse-glimmer:30b-mlx')).toBe('high');
  });
});

describe('The chat model selection is written whole', () => {
  beforeEach(() => {
    setPreferenceMock.mockClear();
    preferences.value = { status: 'loaded', data: { chat_reasoning_efforts: 'claude-opus-5-5=low' } };
    chatModels.value = { status: 'loaded', data: [OPUS_55, LOCAL_MODEL] };
    currentModel.value = DEFAULT_CHAT_MODEL;
  });

  it('stores the tier for the picked model, keeping every other', async () => {
    await setChatModelSelection({ model: 'muse-glimmer:30b-mlx', reasoningEffort: 'high' });

    expect(currentModel.value).toBe('muse-glimmer:30b-mlx');
    expect(setPreferenceMock).toHaveBeenCalledWith(
      'chat_reasoning_efforts', 'claude-opus-5-5=low, muse-glimmer:30b-mlx=high', undefined,
    );
    expect(accountChatEffort('claude-opus-5-5')).toBe('low');
  });

  it('refuses to write the list before preferences load, keeping the stored tiers', async () => {
    preferences.value = { status: 'not-loaded' };

    await setChatModelSelection({ model: 'muse-glimmer:30b-mlx', reasoningEffort: 'high' });

    expect(setPreferenceMock).not.toHaveBeenCalledWith(
      'chat_reasoning_efforts', expect.anything(), expect.anything(),
    );
  });

  it('the Default row removes that model\'s stored tier alone', async () => {
    preferences.value = {
      status: 'loaded',
      data: { chat_reasoning_efforts: 'claude-opus-5-5=low, muse-glimmer:30b-mlx=high' },
    };

    await setChatModelSelection({ model: 'claude-opus-5-5', reasoningEffort: null });

    expect(setPreferenceMock).toHaveBeenCalledWith(
      'chat_reasoning_efforts', 'muse-glimmer:30b-mlx=high', undefined,
    );
    expect(accountChatEffort('claude-opus-5-5')).toBe('medium');
  });

  it('stores nothing for a model with no tiers', async () => {
    await setChatModelSelection({ model: 'imagen-4', reasoningEffort: null });

    expect(setPreferenceMock).not.toHaveBeenCalledWith(
      'chat_reasoning_efforts', expect.anything(), expect.anything(),
    );
  });

  it('the registry arriving supplies the default effort', async () => {
    chatModels.value = { status: 'not-loaded' };
    preferences.value = { status: 'loaded', data: {} };
    expect(accountChatEffort('claude-opus-5-5')).toBeNull();

    listModelsMock.mockResolvedValueOnce({ models: [OPUS_55] });
    await loadChatModels();

    expect(accountChatEffort('claude-opus-5-5')).toBe('medium');
    expect(setPreferenceMock).not.toHaveBeenCalled();
  });
});
