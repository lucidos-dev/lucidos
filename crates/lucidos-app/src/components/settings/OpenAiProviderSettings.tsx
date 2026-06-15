import { useState } from 'preact/hooks';
import { credentials } from '../../store/store';
import { submitNewCredential, deleteCredential } from '../../store/actions/credentials';

const OPENAI_SERVICE = 'openai';
const OPENAI_BASE_URL = 'https://api.openai.com';

/** Configure the direct-OpenAI provider credential (Settings → Models →
 *  Providers). Stores a credential named `openai`; the engine's OpenAiProvider
 *  reads it (preferring it over the OPENAI_API_KEY launch env var) and sends it
 *  as the bearer key. OpenAI authenticates with a single API key, so — unlike
 *  Anthropic — there's no auth-kind choice; the credential is stored as
 *  `api_key`. The secret is write-only — once set we show "configured", never
 *  the value.
 *
 *  Renders only the provider block (label/secret/save rows + note); the
 *  enclosing "Providers" `settings-section` is owned by `SettingsView` so this
 *  and `AnthropicProviderSettings` share one section header. */
export function OpenAiProviderSettings() {
  const credLoadable = credentials.value;
  const existing =
    credLoadable.status === 'loaded'
      ? credLoadable.data.find((c) => c.service_name === OPENAI_SERVICE)
      : undefined;

  const [secret, setSecret] = useState('');
  const [saving, setSaving] = useState(false);

  async function save() {
    if (!secret.trim()) return;
    setSaving(true);
    try {
      const ok = await submitNewCredential(
        OPENAI_SERVICE,
        OPENAI_BASE_URL,
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
          OpenAI (direct)
          {existing && <span class="list-row-details"> · configured</span>}
        </span>
        {existing && (
          <button
            class="action-btn action-btn-danger"
            onClick={() => void deleteCredential(OPENAI_SERVICE)}
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
          placeholder="sk-…"
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
        Direct OpenAI serves models on the <strong>openai</strong> provider (e.g. GPT-5). Stored here,
        the key is used instead of the <strong>OPENAI_API_KEY</strong> launch environment variable,
        which stays as a fallback when no key is set here.
      </div>
    </>
  );
}
