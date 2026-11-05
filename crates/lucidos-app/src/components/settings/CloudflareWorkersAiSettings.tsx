import { useState } from 'preact/hooks';
import { credentials } from '../../store/store';
import { submitNewCredential, deleteCredential } from '../../store/actions/credentials';
import { findProviderCredential } from './providerCredential';
import { SystemOneProviderFrame } from './SystemOneProviderFrame';
import {
  workersAiAccount,
  workersAiScope,
  JUDGMENT_SITE_LOCATION,
  SYSTEM_ONE_PROVIDERS,
} from './judgmentBackend';

const SERVICE = SYSTEM_ONE_PROVIDERS['cloudflare-workers-ai'].credentialService;

/**
 * Cloudflare Workers AI, for Clef and Clef-flash, on Settings → Models →
 * Providers.
 *
 * One credential holds both halves: the token is the secret, and the account
 * rides in its scope URL, which is where the engine reads it from.
 */
export function CloudflareWorkersAiSettings() {
  const existing = findProviderCredential(credentials.value, SERVICE);
  const storedAccount = workersAiAccount(existing?.base_urls[0]);
  const [account, setAccount] = useState('');
  const [token, setToken] = useState('');
  const [saving, setSaving] = useState(false);

  // A replaced token keeps the stored account unless a new one is typed.
  const scope = workersAiScope(account.trim() || storedAccount || '');
  const canSave = !!scope && !!token.trim() && !saving;

  async function save() {
    if (!scope || !token.trim()) return;
    setSaving(true);
    try {
      const ok = await submitNewCredential(SERVICE, [scope], 'api_key', token.trim());
      if (ok) {
        setToken('');
        setAccount('');
      }
    } finally {
      setSaving(false);
    }
  }

  return (
    <SystemOneProviderFrame
      provider="cloudflare-workers-ai"
      explainer={
        <>
          <p>
            Cloudflare's <strong>Clef</strong> and <strong>Clef-flash</strong> answer the
            same typed questions as Jev, on Workers AI. Clef-flash is the fast one.
          </p>
          <p>
            Nothing moves on its own. Choose either as the model in{' '}
            <strong>{JUDGMENT_SITE_LOCATION['command-guard']}</strong> or{' '}
            <strong>{JUDGMENT_SITE_LOCATION['query-classification']}</strong>. A site
            on Clef sends what it judges to Cloudflare. This switch is above both.
          </p>
          <p>
            Use an API token with Workers AI read access, and the account id from
            your Cloudflare dashboard.
          </p>
        </>
      }
      detail={existing ? (
        <span class="list-row-details">
          {storedAccount ? `account ${storedAccount}` : 'configured, no account'}
        </span>
      ) : null}
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
        <span class="settings-row-label">Account ID</span>
        <input
          type="text"
          class="settings-text-input"
          placeholder={storedAccount ?? 'Account ID'}
          value={account}
          onInput={(e) => setAccount((e.target as HTMLInputElement).value)}
        />
      </div>
      <div class="settings-row">
        <span class="settings-row-label">{existing ? 'Replace token' : 'API token'}</span>
        <input
          type="password"
          class="settings-text-input"
          value={token}
          onInput={(e) => setToken((e.target as HTMLInputElement).value)}
        />
      </div>
      <div class="settings-row">
        <span class="settings-row-label" />
        <button
          class="action-btn action-btn-confirm"
          disabled={!canSave}
          onClick={() => void save()}
        >
          {existing ? 'Update' : 'Save'}
        </button>
      </div>
    </SystemOneProviderFrame>
  );
}
