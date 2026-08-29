import { useState } from 'preact/hooks';
import { credentials } from '../../store/store';
import { Dropdown } from '../shared/Dropdown';
import { submitNewCredential, deleteCredential } from '../../store/actions/credentials';
import type { AuthType } from '../../store/types';
import { findProviderCredential } from './providerCredential';
import { ProviderBlock } from './ProviderBlock';

const ANTHROPIC_SERVICE = 'anthropic';
const ANTHROPIC_BASE_URL = 'https://api.anthropic.com';

const AUTH_KINDS = [
  { value: 'api_key', label: 'API key' },
  { value: 'bearer', label: 'OAuth subscription token' },
];

/** Configure the direct-Anthropic provider credential (Settings → Models →
 *  Providers). Stores a credential named `anthropic`; the engine's
 *  AnthropicProvider reads it (preferring it over the ANTHROPIC_API_KEY launch
 *  env var) and picks the header by auth kind (`api_key` → `x-api-key`,
 *  `bearer` → `Authorization: Bearer` + the OAuth beta). The secret
 *  is write-only — once set we show "Configured", never the value.
 *
 *  Renders only the provider block (label/auth/secret/save rows + note); the
 *  enclosing "Providers" `settings-section` is owned by `SettingsView` so this
 *  and `OpenAiProviderSettings` share one section header. */
export function AnthropicProviderSettings() {
  const credLoadable = credentials.value;
  const existing = findProviderCredential(credLoadable, ANTHROPIC_SERVICE);

  const [authKind, setAuthKind] = useState<string>('api_key');
  const [secret, setSecret] = useState('');
  const [saving, setSaving] = useState(false);

  async function save() {
    if (!secret.trim()) return;
    setSaving(true);
    try {
      const ok = await submitNewCredential(
        ANTHROPIC_SERVICE,
        [ANTHROPIC_BASE_URL],
        authKind as AuthType,
        secret.trim()
      );
      if (ok) setSecret('');
    } finally {
      setSaving(false);
    }
  }

  return (
    <ProviderBlock
      id="anthropic"
      label="Anthropic (direct)"
      anchor="models:anthropic"
      hasStoredConfig={!!existing}
      explainer={
        <>
          <p>
            Direct Anthropic serves models on the <strong>anthropic</strong> provider
            (e.g. Fable 5). Stored here, the secret is used instead of the{' '}
            <strong>ANTHROPIC_API_KEY</strong> launch environment variable, which stays
            as a fallback when nothing is set here.
          </p>
          <p>
            OAuth subscription tokens are short-lived: if requests start failing with a
            401, re-paste a fresh token here.
          </p>
        </>
      }
      /* `.list-row-details` is `display: flex`, so this span is a block box
         inside the label's line and renders UNDER it. A manual "·" glue would
         therefore be stranded at the start of that new line, the same artifact
         the rule in `.claude/rules/frontend.md` names. */
      detail={existing && (
        <span class="list-row-details">configured ({existing.auth_type})</span>
      )}
      actions={existing && (
        <button
          class="action-btn action-btn-danger"
          onClick={() => void deleteCredential(existing.id, ANTHROPIC_SERVICE)}
        >
          Remove
        </button>
      )}
    >
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
    </ProviderBlock>
  );
}
