import { useState } from 'preact/hooks';
import { PREF_WEBHOOK_PUBLIC_URL } from '@lucidos/preference-catalog';
import { deletePreference, setPreference } from '../../api/client';
import { loadPreferences } from '../../store/actions/preferences';
import { showToast } from '../../store/store';

export function publicWebhookDeliveryUrl(base: string, deliveryPath: string): string | null {
  try {
    const url = new URL(base);
    if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash) {
      return null;
    }
    return url.origin + url.pathname.replace(/\/+$/, '') + deliveryPath;
  } catch {
    return null;
  }
}

export function WebhookPublicUrlField({ stored }: { stored: string }) {
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const value = draft ?? stored;
  const trimmed = value.trim();
  const changed = trimmed !== stored.trim();

  async function save(e: Event) {
    e.preventDefault();
    if (!changed || saving) return;
    setSaving(true);
    try {
      const result = trimmed
        ? await setPreference(PREF_WEBHOOK_PUBLIC_URL.key, trimmed)
        : await deletePreference(PREF_WEBHOOK_PUBLIC_URL.key);
      if (!result.success) throw new Error(result.error || 'Failed to save the public webhook URL');
      await loadPreferences();
      setDraft(null);
    } catch (error) {
      showToast(error instanceof Error ? error.message : 'Failed to save the public webhook URL', 'error');
    } finally {
      setSaving(false);
    }
  }

  return (
    <form class="settings-row" onSubmit={save}>
      <label class="settings-row-label" for="webhook-public-url">Public webhook URL</label>
      <input
        id="webhook-public-url"
        class="settings-text-input"
        inputMode="url"
        autoCapitalize="off"
        autoCorrect="off"
        spellcheck={false}
        placeholder="https://hooks.example.com"
        aria-describedby="webhook-public-url-help"
        value={value}
        disabled={saving}
        onInput={(e) => {
          const input = e.currentTarget.value;
          setDraft(input === stored ? null : input);
        }}
      />
      <div id="webhook-public-url-help" class="list-row-details list-row-details-prose">
        Enter the public HTTPS base URL that forwards to the hook port. Leave it blank to discover Tailscale Funnel.
        Lucidos checks the route every 15 minutes.
      </div>
      <div class="list-row-actions">
        <button class="action-btn action-btn-confirm" type="submit" disabled={saving || !changed}>
          {saving ? 'Saving…' : 'Save'}
        </button>
      </div>
    </form>
  );
}
