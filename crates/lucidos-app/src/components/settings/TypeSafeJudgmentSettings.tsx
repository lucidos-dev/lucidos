import { useState } from 'preact/hooks';
import { credentials } from '../../store/store';
import { submitNewCredential, deleteCredential } from '../../store/actions/credentials';
import { findProviderCredential } from './providerCredential';
import { SystemOneProviderFrame } from './SystemOneProviderFrame';
import {
  JUDGMENT_SITE_LOCATION,
  SYSTEM_ONE_PROVIDERS,
  TYPESAFE_API_KEY_ENV,
} from './judgmentBackend';

/** The API root the engine's judgment provider posts to. */
const TYPESAFE_BASE_URL = 'https://api.typesafe.ai/v1';

const SERVICE = SYSTEM_ONE_PROVIDERS.typesafe.credentialService;

/** TypeSafe (Jev), on Settings → Models → Providers. */
export function TypeSafeJudgmentSettings() {
  const existing = findProviderCredential(credentials.value, SERVICE);
  const [secret, setSecret] = useState('');
  const [saving, setSaving] = useState(false);

  async function save() {
    if (!secret.trim()) return;
    setSaving(true);
    try {
      const ok = await submitNewCredential(SERVICE, [TYPESAFE_BASE_URL], 'api_key', secret.trim());
      if (ok) setSecret('');
    } finally {
      setSaving(false);
    }
  }

  return (
    <SystemOneProviderFrame
      provider="typesafe"
      explainer={
        <>
          <p>
            Jev answers <strong>typed questions</strong> rather than writing text, so it
            cannot hold a conversation and stays out of the chat model picker. It is
            offered as a model wherever a judgment is picked.
          </p>
          <p>
            Nothing moves on its own. Choose <strong>TypeSafe (Jev)</strong> as the model
            in <strong>{JUDGMENT_SITE_LOCATION['command-guard']}</strong> or{' '}
            <strong>{JUDGMENT_SITE_LOCATION['query-classification']}</strong>. This
            switch is above both: off, every judgment runs on its chat model.
          </p>
          <p>
            A stored key does give the agent a <strong>judge</strong> tool, so it can ask
            Jev a typed question itself when that is the cheaper way to sort, rank or
            filter a lot at once. The switch above turns that off too.
          </p>
          <p>
            Stored here, the key is used instead of the{' '}
            <strong>{TYPESAFE_API_KEY_ENV}</strong> launch environment variable, which
            stays as a fallback. Apps reach the same key through{' '}
            <strong>lucidos.proxy('typesafe')</strong>, so an app never holds it.
          </p>
        </>
      }
      detail={existing ? <span class="list-row-details">configured</span> : null}
      actions={existing && (
        <button
          class="action-btn action-btn-danger"
          onClick={() => void deleteCredential(existing.id, SERVICE)}
        >
          Remove
        </button>
      )}
      hasStoredConfig={!!existing}
    >
      <div class="settings-row">
        <span class="settings-row-label">{existing ? 'Replace secret' : 'Secret'}</span>
        <input
          type="password"
          class="settings-text-input"
          placeholder="ts-…"
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
    </SystemOneProviderFrame>
  );
}
