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

import { reasoningEffort, currentModel, preferences, chatModels } from '../../../store/store';
import { setChatModelSelection, setReasoningEffort, loadPreferences } from '../../../store/actions/preferences';
import { loadChatModels } from '../../../store/actions/models';
import { DEFAULT_CHAT_MODEL } from '../../../store/models';
import type { ModelInfo } from '../../../api/types';

/** A `provider = local` registry row as the engine serves it: the tiers come
 *  from `llm::reasoning::supported_efforts`, which stops local servers at
 *  `high` because `xhigh` is OpenAI-proprietary. */
const LOCAL_MODEL: ModelInfo = {
  id: 'muse-glimmer:30b-mlx',
  label: 'Muse Glimmer 30B (local)',
  provider: 'local',
  sort_order: 1000,
  source: 'user',
  enabled: true,
  context_window: 131072,
  created_at: '2026-01-01T00:00:00Z',
  reasoning_efforts: ['none', 'low', 'medium', 'high'],
};

describe('Chat model and reasoning effort persist across restarts', () => {
  beforeEach(() => {
    setPreferenceMock.mockClear();
    getPreferencesMock.mockClear();
    listModelsMock.mockClear();
    preferences.value = { status: 'loaded', data: {} };
    chatModels.value = { status: 'not-loaded' };
    currentModel.value = DEFAULT_CHAT_MODEL;
    reasoningEffort.value = 'high';
  });

  it('setChatModelSelection writes the model preference to the API', async () => {
    await setChatModelSelection({ model: 'claude-sonnet-4-6', reasoningEffort: 'high' });
    expect(currentModel.value).toBe('claude-sonnet-4-6');
    expect(setPreferenceMock).toHaveBeenCalledWith('chat_model', 'claude-sonnet-4-6', undefined);
  });

  it('setReasoningEffort writes the effort preference to the API', async () => {
    await setReasoningEffort('low');
    expect(reasoningEffort.value).toBe('low');
    expect(setPreferenceMock).toHaveBeenCalledWith('chat_reasoning_effort', 'low', undefined);
  });

  it('loadPreferences restores saved model and effort (simulated restart)', async () => {
    getPreferencesMock.mockResolvedValueOnce({
      preferences: { chat_model: 'gemini-3.1-pro-preview', chat_reasoning_effort: 'medium' },
    });
    // Simulate fresh app: signals at defaults, preferences not loaded yet.
    currentModel.value = DEFAULT_CHAT_MODEL;
    reasoningEffort.value = 'high';
    preferences.value = { status: 'not-loaded' };

    await loadPreferences();

    expect(currentModel.value).toBe('gemini-3.1-pro-preview');
    expect(reasoningEffort.value).toBe('medium');
  });

  it('loadPreferences leaves defaults when no preference saved', async () => {
    getPreferencesMock.mockResolvedValueOnce({ preferences: {} });
    currentModel.value = DEFAULT_CHAT_MODEL;
    reasoningEffort.value = 'high';
    preferences.value = { status: 'not-loaded' };

    await loadPreferences();

    expect(currentModel.value).toBe(DEFAULT_CHAT_MODEL);
    expect(reasoningEffort.value).toBe('high');
  });

  it('loadPreferences honors any stored model value (registry is user-extensible)', async () => {
    // The model set is now the DB-backed registry (users add their own), so the
    // frontend no longer validates `chat_model` against a fixed allow-list — it
    // honors the stored value and RoutingProvider resolves it (with a prefix
    // fallback). An unknown reasoning effort still falls back to 'high'.
    getPreferencesMock.mockResolvedValueOnce({
      preferences: { chat_model: 'my-custom-model', chat_reasoning_effort: 'fake-effort' },
    });
    currentModel.value = DEFAULT_CHAT_MODEL;
    reasoningEffort.value = 'high';
    preferences.value = { status: 'not-loaded' };

    await loadPreferences();

    expect(currentModel.value).toBe('my-custom-model');
    expect(reasoningEffort.value).toBe('high');
  });

  it('loadPreferences falls back to the default model when none is stored', async () => {
    getPreferencesMock.mockResolvedValueOnce({ preferences: { chat_reasoning_effort: 'medium' } });
    currentModel.value = 'something-else';
    reasoningEffort.value = 'high';
    preferences.value = { status: 'not-loaded' };

    await loadPreferences();

    expect(currentModel.value).toBe(DEFAULT_CHAT_MODEL);
    expect(reasoningEffort.value).toBe('medium');
  });
});

// One pick sets the pair, so the effort is the user's own choice rather than a
// clamp of the previous one. What still clamps is a STORED pair the registry
// later contradicts, which is the fault reported here: switching to a local
// model with the account effort at xhigh sent a tier the local server had never
// heard of, and the turn 400'd.
describe('The chat model selection is written whole', () => {
  beforeEach(() => {
    setPreferenceMock.mockClear();
    getPreferencesMock.mockClear();
    listModelsMock.mockClear();
    preferences.value = { status: 'loaded', data: {} };
    chatModels.value = { status: 'not-loaded' };
    currentModel.value = DEFAULT_CHAT_MODEL;
    reasoningEffort.value = 'high';
  });

  it('persists both halves of a pick', async () => {
    chatModels.value = { status: 'loaded', data: [LOCAL_MODEL] };
    reasoningEffort.value = 'xhigh';

    await setChatModelSelection({ model: 'muse-glimmer:30b-mlx', reasoningEffort: 'high' });

    expect(currentModel.value).toBe('muse-glimmer:30b-mlx');
    expect(reasoningEffort.value).toBe('high');
    expect(setPreferenceMock).toHaveBeenCalledWith('chat_reasoning_effort', 'high', undefined);
  });

  it('leaves the stored effort alone for a model with no tiers', async () => {
    // An image model reports `null`. `RoutingProvider::effort_for_model` drops
    // what the model cannot take, so there is nothing to write.
    reasoningEffort.value = 'medium';

    await setChatModelSelection({ model: 'imagen-4', reasoningEffort: null });

    expect(reasoningEffort.value).toBe('medium');
    expect(setPreferenceMock).not.toHaveBeenCalledWith(
      'chat_reasoning_effort', expect.anything(), expect.anything(),
    );
  });

  // Ordering: preferences land before the registry, so the first clamp runs on
  // the id-shape heuristic, which has no rule for a local id and lets `max`
  // through. The registry's arrival has to correct it, or the picker keeps
  // displaying a tier the engine would silently snap on the way to the wire.
  it('the registry arriving re-clamps an effort the heuristic let through', async () => {
    getPreferencesMock.mockResolvedValueOnce({
      preferences: { chat_model: 'muse-glimmer:30b-mlx', chat_reasoning_effort: 'max' },
    });
    preferences.value = { status: 'not-loaded' };
    await loadPreferences();
    expect(reasoningEffort.value).toBe('max');

    listModelsMock.mockResolvedValueOnce({ models: [LOCAL_MODEL] });
    await loadChatModels();

    expect(reasoningEffort.value).toBe('high');
    // Display only: the stored preference is not rewritten, since the model
    // this was clamped against may itself change again.
    expect(setPreferenceMock).not.toHaveBeenCalled();
  });
});
