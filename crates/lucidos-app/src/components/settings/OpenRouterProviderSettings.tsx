import { useState } from 'preact/hooks';
import { credentials } from '../../store/store';
import { submitNewCredential, deleteCredential } from '../../store/actions/credentials';

const OPENROUTER_SERVICE = 'openrouter';
const OPENROUTER_BASE_URL = 'https://openrouter.ai/api/v1';

/** Configure the OpenRouter provider credential (Settings → Models →
 *  Providers). Stores a credential named `openrouter`; the engine builds an
 *  OpenAI-compatible provider pointed at openrouter.ai and sends it as the
 *  bearer key (preferring it over the LUCIDOS_OPENROUTER_API_KEY launch env
 *  var). OpenRouter authenticates with a single API key, so — like OpenAI —
 *  there's no auth-kind choice; stored as `api_key`. The secret is write-only —
 *  once set we show "configured", never the value.
 *
 *  Renders only the provider block; the enclosing "Providers" `settings-section`
 *  is owned by `SettingsView`. */
export function OpenRouterProviderSettings() {
  const credLoadable = credentials.value;
  const existing =
    credLoadable.status === 'loaded'
      ? credLoadable.data.find((c) => c.service_name === OPENROUTER_SERVICE)
      : undefined;

  const [secret, setSecret] = useState('');
  const [saving, setSaving] = useState(false);

  async function save() {
    if (!secret.trim()) return;
    setSaving(true);
    try {
      const ok = await submitNewCredential(
        OPENROUTER_SERVICE,
        OPENROUTER_BASE_URL,
        'api_key',
        secret.trim()
      );
      if (ok) setSecret('');
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <div class="settings-row">
        <span class="settings-row-label">
          OpenRouter
          {existing && <span class="list-row-details"> · configured</span>}
        </span>
        {existing && (
          <button
            class="action-btn action-btn-danger"
            onClick={() => void deleteCredential(OPENROUTER_SERVICE)}
          >
            Remove
          </button>
        )}
      </div>
      <div class="settings-row">
        <span class="settings-row-label">{existing ? 'Replace secret' : 'Secret'}</span>
        <input
          type="password"
          class="settings-text-input"
          placeholder="sk-or-…"
          value={secret}
          onInput={(e) => setSecret((e.target as HTMLInputElement).value)}
        />
      </div>
      <div class="settings-row">
        <span class="settings-row-label" />
        <button
          class="action-btn action-btn-confirm"
          disabled={saving || !secret.trim()}
          onClick={() => void save()}
        >
          {existing ? 'Update' : 'Save'}
        </button>
      </div>
      <div class="settings-row-note">
        OpenRouter serves models on the <strong>openrouter</strong> provider (e.g. GLM 5.2).
        Stored here, the key is used instead of the <strong>LUCIDOS_OPENROUTER_API_KEY</strong> launch
        environment variable, which stays as a fallback. Adding the credential takes effect on the next
        engine restart.
      </div>
    </>
  );
}
