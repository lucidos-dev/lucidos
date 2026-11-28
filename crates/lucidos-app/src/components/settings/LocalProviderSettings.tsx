import { useEffect, useState } from 'preact/hooks';
import { credentials, preferences } from '../../store/store';
import { submitNewCredential, submitCredentialEdit, deleteCredential } from '../../store/actions/credentials';
import {
  currentLocalBaseUrl,
  setLocalBaseUrl,
  DEFAULT_LOCAL_BASE_URL,
} from '../../store/actions/preferences';
import { useServerBackedField } from '../../hooks/useServerBackedField';
import { findProviderCredential, providerKeyRescope } from './providerCredential';
import { ProviderBlock } from './ProviderBlock';
import { providerAnchor } from '../../store/modelProviders';

const LOCAL_SERVICE = 'local';

export function LocalProviderSettings() {
  preferences.value;
  const credLoadable = credentials.value;
  const existing = findProviderCredential(credLoadable, LOCAL_SERVICE);

  const saved = currentLocalBaseUrl();
  const [url, setUrl] = useServerBackedField(saved);
  useEffect(() => {
    if (url.trim() === saved.trim()) setUrl(saved);
  }, [saved, url]);

  const [secret, setSecret] = useState('');
  const [savingUrl, setSavingUrl] = useState(false);
  const [savingKey, setSavingKey] = useState(false);

  async function saveUrl() {
    setSavingUrl(true);
    try {
      await setLocalBaseUrl(url);
      const rescope = providerKeyRescope(existing, url);
      if (rescope && existing) await submitCredentialEdit(existing.id, rescope);
    } finally {
      setSavingUrl(false);
    }
  }

  async function saveKey() {
    if (!secret.trim()) return;
    setSavingKey(true);
    try {
      const ok = await submitNewCredential(
        LOCAL_SERVICE,
        [url.trim() || DEFAULT_LOCAL_BASE_URL],
        'api_key',
        secret.trim()
      );
      if (ok) setSecret('');
    } finally {
      setSavingKey(false);
    }
  }

  return (
    <ProviderBlock
      id="local"
      label="Local (OpenAI-compatible)"
      anchor={providerAnchor('local')}
      hasStoredConfig={!!existing}
      explainer={
        <>
          <p>
            Serves models on the <strong>local</strong> provider via any
            OpenAI-compatible server: Ollama (default{' '}
            <strong>{DEFAULT_LOCAL_BASE_URL}</strong>), LM Studio, vLLM, llama.cpp.
          </p>
          <p>
            Add your local models in <strong>Manage Models</strong> with the{' '}
            <strong>local</strong> provider, using the model id the server exposes
            (e.g. <code>llama3.1</code>).
          </p>
          <p>
            Also settable via the <strong>LUCIDOS_LOCAL_BASE_URL</strong> /{' '}
            <strong>LUCIDOS_LOCAL_API_KEY</strong> launch env vars.
          </p>
        </>
      }
      detail={existing && <span class="list-row-details">key configured</span>}
      actions={existing && (
        <button
          class="action-btn action-btn-danger"
          onClick={() => void deleteCredential(existing.id, LOCAL_SERVICE)}
        >
          Remove key
        </button>
      )}
    >
      <div class="settings-row">
        <span class="settings-row-label">Base URL</span>
        <input
          type="text"
          class="settings-text-input"
          placeholder={DEFAULT_LOCAL_BASE_URL}
          value={url}
          onInput={(e) => setUrl((e.target as HTMLInputElement).value)}
        />
      </div>
      <div class="settings-row">
        <span class="settings-row-label" />
        <button
          class="action-btn action-btn-confirm"
          disabled={savingUrl || url.trim() === saved.trim()}
          onClick={() => void saveUrl()}
        >
          Save base URL
        </button>
      </div>
      <div class="settings-row">
        <span class="settings-row-label">{existing ? 'Replace key' : 'API key (optional)'}</span>
        <input
          type="password"
          class="settings-text-input"
          placeholder="usually empty for local"
          value={secret}
          onInput={(e) => setSecret((e.target as HTMLInputElement).value)}
        />
      </div>
      <div class="settings-row">
        <span class="settings-row-label" />
        <button
          class="action-btn action-btn-confirm"
          disabled={savingKey || !secret.trim()}
          onClick={() => void saveKey()}
        >
          {existing ? 'Update key' : 'Save key'}
        </button>
      </div>
    </ProviderBlock>
  );
}
