import { useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { configuredProviders, credentials, preferences } from '../../store/store';
import { submitNewCredential, deleteCredential } from '../../store/actions/credentials';
import type { SwitchableProvider } from '../../store/actions/preferences';
import { findProviderCredential } from './providerCredential';
import { ProviderBlock } from './ProviderBlock';
import { SystemOneUsageRow } from './SystemOneProviderFrame';
import {
  SYSTEM_ONE_ENDPOINTS,
  systemOneConsumers,
  type SystemOneProviderId,
} from './judgmentBackend';

/** Shared block for a provider that authenticates with a single API key stored
 *  as an `api_key` credential (OpenAI, OpenRouter, xAI). The secret is
 *  write-only: once set we show "configured", never the value. Renders only the provider
 *  rows; the enclosing "Providers" `settings-section` is owned by
 *  `SettingsView`. Providers with extra knobs (Anthropic's auth kind, Local's
 *  base URL) keep their own components. */
export function ApiKeyProviderSettings({ service, baseUrl, label, placeholder, note, systemOne, children }: {
  /** Credential service name the engine reads. It doubles as the provider id,
   *  which is what the enable switch and `/health` are keyed on. */
  service: SwitchableProvider;
  baseUrl: string;
  /** Row label, e.g. "OpenAI (direct)". */
  label: string;
  placeholder: string;
  /** Provider-specific explanation, behind the provider row's *explainer*
   *  rather than a paragraph under the rows: it is static reference material
   *  read once, and the Providers page is otherwise a stack of such
   *  paragraphs with the actual fields lost between them. */
  note: ComponentChildren;
  /** The System One row that reuses this provider's key, if any. Its block
   *  then says which judgments run on it. */
  systemOne?: SystemOneProviderId;
  /** Rows of that System One use, under the key rows. */
  children?: ComponentChildren;
}) {
  const credLoadable = credentials.value;
  const existing = findProviderCredential(credLoadable, service);

  const [secret, setSecret] = useState('');
  const [saving, setSaving] = useState(false);

  async function save() {
    if (!secret.trim()) return;
    setSaving(true);
    try {
      const ok = await submitNewCredential(service, [baseUrl], 'api_key', secret.trim());
      if (ok) setSecret('');
    } finally {
      setSaving(false);
    }
  }

  return (
    <ProviderBlock
      id={service}
      label={label}
      anchor={`models:${service}`}
      explainer={note}
      hasStoredConfig={!!existing}
      /* `.list-row-details` is `display: flex`, so this span is a block box
         inside the label's line and renders UNDER it. A manual "·" glue would
         therefore be stranded at the start of that new line, the same artifact
         the rule in `.claude/rules/frontend.md` names. */
      detail={existing ? <span class="list-row-details">configured</span> : null}
      actions={existing && (
        <button
          class="action-btn action-btn-danger"
          onClick={() => void deleteCredential(existing.id, service)}
        >
          Remove
        </button>
      )}
    >
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
      {children}
      {systemOne && (
        <SystemOneUsageRow
          anchor={`models:${systemOne}-usage`}
          inUseBy={systemOneConsumers(
            systemOne,
            preferences.value,
            !!existing,
            configuredProviders.value,
          )}
          empty={`No judgment yet. ${systemOneRowLabel(systemOne)} is offered in each judgment's model control.`}
        />
      )}
    </ProviderBlock>
  );
}

/** The picker label of a provider's System One row. */
function systemOneRowLabel(provider: SystemOneProviderId): string {
  return SYSTEM_ONE_ENDPOINTS.find((e) => e.provider === provider)?.choice.label ?? provider;
}
