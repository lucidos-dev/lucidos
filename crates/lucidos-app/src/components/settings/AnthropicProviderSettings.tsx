import { useState } from 'preact/hooks';
import { credentials } from '../../store/store';
import { Dropdown } from '../shared/Dropdown';
import { submitNewCredential, deleteCredential } from '../../store/actions/credentials';
import type { AuthType } from '../../store/types';

const ANTHROPIC_SERVICE = 'anthropic';
const ANTHROPIC_BASE_URL = 'https://api.anthropic.com';

const AUTH_KINDS = [
  { value: 'api_key', label: 'API key' },
  { value: 'bearer', label: 'OAuth subscription token' },
];

/** Configure the direct-Anthropic provider credential (Settings → Models →
 *  Providers). Stores a credential named `anthropic`; the engine's
 *  AnthropicProvider reads it and picks the header by auth kind (`api_key` →
 *  `x-api-key`, `bearer` → `Authorization: Bearer` + the OAuth beta). The secret
 *  is write-only — once set we show "Configured", never the value.
 *
 *  Renders only the provider block (label/auth/secret/save rows + note); the
 *  enclosing "Providers" `settings-section` is owned by `SettingsView` so this
 *  and `OpenAiProviderSettings` share one section header. */
export function AnthropicProviderSettings() {
  const credLoadable = credentials.value;
  const existing =
    credLoadable.status === 'loaded'
      ? credLoadable.data.find((c) => c.service_name === ANTHROPIC_SERVICE)
      : undefined;

  const [authKind, setAuthKind] = useState<string>('api_key');
  const [secret, setSecret] = useState('');
  const [saving, setSaving] = useState(false);

  async function save() {
    if (!secret.trim()) return;
    setSaving(true);
    try {
      const ok = await submitNewCredential(
        ANTHROPIC_SERVICE,
        ANTHROPIC_BASE_URL,
        authKind as AuthType,
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
          Anthropic (direct)
          {existing && (
            <span class="list-row-details"> · configured ({existing.auth_type})</span>
          )}
        </span>
        {existing && (
          <button
            class="action-btn action-btn-danger"
            onClick={() => void deleteCredential(ANTHROPIC_SERVICE)}
          >
            Remove
          </button>
        )}
      </div>
      <div class="settings-row">
        <span class="settings-row-label">Auth</span>
        <Dropdown options={AUTH_KINDS} value={authKind} onChange={setAuthKind} />
      </div>
      <div class="settings-row">
        <span class="settings-row-label">{existing ? 'Replace secret' : 'Secret'}</span>
        <input
          type="password"
          class="settings-text-input"
          placeholder={authKind === 'api_key' ? 'sk-ant-…' : 'OAuth token'}
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
        Direct Anthropic serves models on the <strong>anthropic</strong> provider (e.g. Fable 5).
        OAuth subscription tokens are short-lived — if requests start failing with a 401, re-paste a
        fresh token here.
      </div>
    </>
  );
}
