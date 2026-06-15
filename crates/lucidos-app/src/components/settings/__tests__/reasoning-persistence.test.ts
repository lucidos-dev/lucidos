import { describe, it, expect, vi, beforeEach } from 'vitest';

const setPreferenceMock = vi.fn(async (..._args: unknown[]) => ({ ok: true }));
const getPreferencesMock = vi.fn(async (..._args: unknown[]) => ({
  preferences: {} as Record<string, string>,
}));

vi.mock('../../../api/client', () => ({
  setPreference: (key: string, value: string, deviceId?: string) =>
    setPreferenceMock(key, value, deviceId),
  getPreferences: (deviceId?: string) => getPreferencesMock(deviceId),
}));

import { reasoningEffort, currentModel, preferences } from '../../../store/store';
import { setCurrentModel, setReasoningEffort, loadPreferences } from '../../../store/actions/preferences';
import { DEFAULT_CHAT_MODEL } from '../../../store/models';

describe('Chat model and reasoning effort persist across restarts', () => {
  beforeEach(() => {
    setPreferenceMock.mockClear();
    getPreferencesMock.mockClear();
    preferences.value = { status: 'loaded', data: {} };
    currentModel.value = DEFAULT_CHAT_MODEL;
    reasoningEffort.value = 'high';
  });

  it('setCurrentModel writes the model preference to the API', async () => {
    await setCurrentModel('claude-sonnet-4-6');
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
