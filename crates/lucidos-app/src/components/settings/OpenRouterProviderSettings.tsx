import { useEffect, useState } from 'preact/hooks';
import { preferences } from '../../store/store';
import { savePreference, SYSTEM_ONE_OPENROUTER_MODEL_KEY } from '../../store/actions/preferences';
import { useServerBackedField } from '../../hooks/useServerBackedField';
import { ApiKeyProviderSettings } from './ApiKeyProviderSettings';
import { OPENROUTER_JEV_MODEL } from '@lucidos/engine-constants';

export function OpenRouterProviderSettings() {
  const prefs = preferences.value;
  const savedModel = prefs.status === 'loaded' ? prefs.data[SYSTEM_ONE_OPENROUTER_MODEL_KEY] ?? '' : '';
  const [model, setModel] = useServerBackedField(savedModel);
  useEffect(() => {
    if (model.trim() === savedModel.trim()) setModel(savedModel);
  }, [savedModel, model]);
  const [saving, setSaving] = useState(false);

  async function saveModel() {
    setSaving(true);
    try {
      await savePreference(SYSTEM_ONE_OPENROUTER_MODEL_KEY, model.trim());
    } finally {
      setSaving(false);
    }
  }

  return (
    <ApiKeyProviderSettings
      service="openrouter"
      baseUrl="https://openrouter.ai/api/v1"
      label="OpenRouter"
      placeholder="sk-or-…"
      systemOne="openrouter"
      note={
        <>
          OpenRouter serves models on the <strong>openrouter</strong> provider (e.g. GLM 5.2).
          Stored here, the key is used instead of the <strong>LUCIDOS_OPENROUTER_API_KEY</strong> launch
          environment variable, which stays as a fallback. Adding the credential takes effect on the next
          engine restart.
          {' '}The same key answers judgments through <strong>OpenRouter System One</strong> when a
          judgment's model control picks it. It asks Jev unless the System One model below names another.
        </>
      }
    >
      <div class="settings-row">
        <span class="settings-row-label">System One model</span>
        <input
          type="text"
          class="settings-text-input"
          placeholder={OPENROUTER_JEV_MODEL}
          value={model}
          onInput={(e) => setModel((e.target as HTMLInputElement).value)}
        />
      </div>
      <div class="settings-row">
        <span class="settings-row-label" />
        <button
          class="action-btn action-btn-confirm"
          disabled={saving || model.trim() === savedModel.trim()}
          onClick={() => void saveModel()}
        >
          Save model
        </button>
      </div>
    </ApiKeyProviderSettings>
  );
}
