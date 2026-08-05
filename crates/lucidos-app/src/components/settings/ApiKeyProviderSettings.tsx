import { useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { credentials } from '../../store/store';
import { submitNewCredential, deleteCredential } from '../../store/actions/credentials';
import { findProviderCredential } from './providerCredential';

/** Shared block for a provider that authenticates with a single API key stored
 *  as an `api_key` credential (OpenAI, OpenRouter). The secret is write-only —
 *  once set we show "configured", never the value. Renders only the provider
 *  rows + note; the enclosing "Providers" `settings-section` is owned by
 *  `SettingsView`. Providers with extra knobs (Anthropic's auth kind, Local's
 *  base URL) keep their own components. */
export function ApiKeyProviderSettings({ service, baseUrl, label, placeholder, note }: {
  /** Credential service name the engine reads (`openai`, `openrouter`). */
  service: string;
  baseUrl: string;
  /** Row label, e.g. "OpenAI (direct)". */
  label: string;
  placeholder: string;
  /** Provider-specific explanatory note rendered under the rows. */
  note: ComponentChildren;
}) {
  const credLoadable = credentials.value;
  const existing = findProviderCredential(credLoadable, service);

  const [secret, setSecret] = useState('');
  const [saving, setSaving] = useState(false);

  async function save() {
    if (!secret.trim()) return;
    setSaving(true);
    try {
      const ok = await submitNewCredential(service, baseUrl, 'api_key', secret.trim());
      if (ok) setSecret('');
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <div class="settings-row">
        <span class="settings-row-label">
          {label}
          {/* `.list-row-details` is `display: flex`, so this span is a block box
              inside the label's line and renders UNDER it. A manual "·" glue
              would therefore be stranded at the start of that new line, the
              same artifact the rule in `.claude/rules/frontend.md` names. */}
          {existing && <span class="list-row-details">configured</span>}
        </span>
        {existing && (
          <button
            class="action-btn action-btn-danger"
            onClick={() => void deleteCredential(existing.id, service)}
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
          placeholder={placeholder}
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
      <div class="settings-row-note">{note}</div>
    </>
  );
}
