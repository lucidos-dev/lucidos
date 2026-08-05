import { useState, useEffect } from 'preact/hooks';
import { credentials, preferences } from '../../store/store';
import { submitNewCredential, deleteCredential } from '../../store/actions/credentials';
import {
  currentLocalBaseUrl,
  setLocalBaseUrl,
  DEFAULT_LOCAL_BASE_URL,
} from '../../store/actions/preferences';
import { findProviderCredential } from './providerCredential';

const LOCAL_SERVICE = 'local';

/** Configure the local OpenAI-compatible provider (Settings → Models →
 *  Providers): Ollama / LM Studio / vLLM / llama.cpp. The base URL is stored as
 *  the `local_base_url` preference; an optional API key is stored as a `local`
 *  credential (most local servers ignore it — the engine omits the
 *  Authorization header when it's empty). The engine builds the provider only
 *  when a base URL or key is configured, and the change takes effect on the next
 *  engine restart.
 *
 *  Renders only the provider block; the enclosing "Providers" `settings-section`
 *  is owned by `SettingsView`. */
export function LocalProviderSettings() {
  // Subscribe to preference + credential signals.
  preferences.value;
  const credLoadable = credentials.value;
  const existing = findProviderCredential(credLoadable, LOCAL_SERVICE);

  const saved = currentLocalBaseUrl();
  const [url, setUrl] = useState(saved);
  // Re-sync the field when the persisted value changes (async load / after save).
  useEffect(() => {
    setUrl(saved);
  }, [saved]);

  const [secret, setSecret] = useState('');
  const [savingUrl, setSavingUrl] = useState(false);
  const [savingKey, setSavingKey] = useState(false);

  async function saveUrl() {
    setSavingUrl(true);
    try {
      await setLocalBaseUrl(url);
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
        url.trim() || DEFAULT_LOCAL_BASE_URL,
        'api_key',
        secret.trim()
      );
      if (ok) setSecret('');
    } finally {
      setSavingKey(false);
    }
  }

  return (
    <>
      <div class="settings-row">
        <span class="settings-row-label">
          Local (OpenAI-compatible)
          {/* `.list-row-details` is `display: flex`, so this span is a block box
              inside the label's line and renders UNDER it. A manual "·" glue
              would therefore be stranded at the start of that new line, the
              same artifact the rule in `.claude/rules/frontend.md` names. */}
          {existing && <span class="list-row-details">key configured</span>}
        </span>
        {existing && (
          <button
            class="action-btn action-btn-danger"
            onClick={() => void deleteCredential(existing.id, LOCAL_SERVICE)}
          >
            Remove key
          </button>
        )}
      </div>
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
      <div class="settings-row-note">
        Serves models on the <strong>local</strong> provider via any OpenAI-compatible server — Ollama
        (default <strong>{DEFAULT_LOCAL_BASE_URL}</strong>), LM Studio, vLLM, llama.cpp. Add your local
        models in <strong>Manage Models</strong> with the <strong>local</strong> provider, using the
        model id the server exposes (e.g. <code>llama3.1</code>). Also settable via the
        <strong> LUCIDOS_LOCAL_BASE_URL</strong> / <strong>LUCIDOS_LOCAL_API_KEY</strong> launch env vars.
      </div>
    </>
  );
}
