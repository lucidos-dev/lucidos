import { useEffect, useState } from 'preact/hooks';
import { credentials, preferences } from '../../store/store';
import {
  submitNewCredential,
  submitCredentialEdit,
  deleteCredential,
} from '../../store/actions/credentials';
import {
  savePreference,
  SYSTEM_ONE_CUSTOM_MODEL_KEY,
  SYSTEM_ONE_CUSTOM_URL_KEY,
} from '../../store/actions/preferences';
import { useServerBackedField } from '../../hooks/useServerBackedField';
import { findProviderCredential, providerKeyRescope } from './providerCredential';
import { SystemOneProviderFrame } from './SystemOneProviderFrame';
import { JUDGMENT_SITE_LOCATION, SYSTEM_ONE_PROVIDERS } from './judgmentBackend';

const SERVICE = SYSTEM_ONE_PROVIDERS['system-one-custom'].credentialService;

/** A stored preference's value, or empty while preferences load. */
function stored(key: string): string {
  const prefs = preferences.value;
  return prefs.status === 'loaded' ? prefs.data[key] ?? '' : '';
}

/** Whether a URL is one the engine will post to: http(s) and nothing else. */
export function customEndpointUrlValid(url: string): boolean {
  return /^https?:\/\/\S+$/.test(url.trim());
}

/**
 * A custom System One endpoint on Settings → Models → Providers, such as an
 * open model served on this machine.
 *
 * The URL and the model are preferences, and the key is an optional
 * credential, the shape the Local chat provider already has. The engine reads
 * all three per judgment, so nothing needs a restart.
 */
export function CustomSystemOneSettings() {
  const existing = findProviderCredential(credentials.value, SERVICE);
  const savedUrl = stored(SYSTEM_ONE_CUSTOM_URL_KEY);
  const savedModel = stored(SYSTEM_ONE_CUSTOM_MODEL_KEY);
  // Server-backed, so a frame from another device repaints an untouched field
  // and a draft survives one (ADR 0118).
  const [url, setUrl] = useServerBackedField(savedUrl);
  const [model, setModel] = useServerBackedField(savedModel);
  // Re-arm each field once our own save lands. This page stays mounted, so
  // without it the field ignores every later frame.
  useEffect(() => {
    if (url.trim() === savedUrl.trim()) setUrl(savedUrl);
  }, [savedUrl, url]);
  useEffect(() => {
    if (model.trim() === savedModel.trim()) setModel(savedModel);
  }, [savedModel, model]);

  const [secret, setSecret] = useState('');
  const [savingEndpoint, setSavingEndpoint] = useState(false);
  const [savingKey, setSavingKey] = useState(false);

  const unchanged = url.trim() === savedUrl.trim() && model.trim() === savedModel.trim();
  const endpointValid = customEndpointUrlValid(url) && !!model.trim();

  async function saveEndpoint() {
    setSavingEndpoint(true);
    try {
      await savePreference(SYSTEM_ONE_CUSTOM_URL_KEY, url.trim());
      await savePreference(SYSTEM_ONE_CUSTOM_MODEL_KEY, model.trim());
      // The engine sends the key only inside its scope, so it moves with the URL.
      const rescope = providerKeyRescope(existing, url);
      if (rescope && existing) await submitCredentialEdit(existing.id, rescope);
    } finally {
      setSavingEndpoint(false);
    }
  }

  async function saveKey() {
    if (!secret.trim() || !customEndpointUrlValid(url)) return;
    setSavingKey(true);
    try {
      const ok = await submitNewCredential(SERVICE, [url.trim()], 'api_key', secret.trim());
      if (ok) setSecret('');
    } finally {
      setSavingKey(false);
    }
  }

  return (
    <SystemOneProviderFrame
      provider="system-one-custom"
      explainer={
        <>
          <p>
            Any endpoint that speaks the System One API, such as an open decision model
            you host yourself. A self-hosted model keeps every judgment on your own
            machine.
          </p>
          <p>
            Give the full request URL and the model name it expects. The key is
            optional, because a local server often takes none.
          </p>
          <p>
            Nothing moves on its own. Choose <strong>Custom System One</strong> as the
            model in <strong>{JUDGMENT_SITE_LOCATION['command-guard']}</strong> or{' '}
            <strong>{JUDGMENT_SITE_LOCATION['query-classification']}</strong>.
          </p>
        </>
      }
      detail={savedUrl ? <span class="list-row-details">{savedModel || 'no model'}</span> : null}
      actions={existing && (
        <button
          class="action-btn action-btn-danger"
          onClick={() => void deleteCredential(existing.id, SERVICE)}
        >
          Remove key
        </button>
      )}
      hasStoredConfig={!!existing}
    >
      <div class="settings-row">
        <span class="settings-row-label">Endpoint URL</span>
        <input
          type="text"
          class="settings-text-input"
          placeholder="http://localhost:8000/v1/systemone"
          value={url}
          onInput={(e) => setUrl((e.target as HTMLInputElement).value)}
        />
      </div>
      <div class="settings-row">
        <span class="settings-row-label">Model</span>
        <input
          type="text"
          class="settings-text-input"
          placeholder="kev"
          value={model}
          onInput={(e) => setModel((e.target as HTMLInputElement).value)}
        />
      </div>
      <div class="settings-row">
        <span class="settings-row-label" />
        <button
          class="action-btn action-btn-confirm"
          disabled={savingEndpoint || unchanged || !endpointValid}
          onClick={() => void saveEndpoint()}
        >
          Save endpoint
        </button>
      </div>
      <div class="settings-row">
        <span class="settings-row-label">{existing ? 'Replace key' : 'API key (optional)'}</span>
        <input
          type="password"
          class="settings-text-input"
          value={secret}
          onInput={(e) => setSecret((e.target as HTMLInputElement).value)}
        />
      </div>
      <div class="settings-row">
        <span class="settings-row-label" />
        <button
          class="action-btn action-btn-confirm"
          disabled={savingKey || !secret.trim() || !customEndpointUrlValid(url)}
          onClick={() => void saveKey()}
        >
          {existing ? 'Update key' : 'Save key'}
        </button>
      </div>
    </SystemOneProviderFrame>
  );
}
