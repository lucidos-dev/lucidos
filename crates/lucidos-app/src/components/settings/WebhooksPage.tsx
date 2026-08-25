import { useEffect, useState } from 'preact/hooks';
import {
  createWebhook,
  deleteWebhook,
  fetchWebhooks,
  updateWebhook,
  type Webhook,
} from '../../api/client';
import { ListRowAddCard } from '../shared/ListRowAddCard';
import { LoadableError } from '../shared/LoadableError';
import { showToast, webhooksVersion } from '../../store/store';
import { toFailed, loadingIfFresh, type Loadable } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';

/** The event a webhook emits, as the user must type it. */
const EVENT_TYPE_HINT =
  'PascalCase, past tense, e.g. DeployFinished. Pinned: a caller cannot change it, so one endpoint can only ever fire this event.';

/** The form for a new webhook. Signature config is deliberately absent: it is a
 *  nine-field object per sender, so it is set from the CLI, and the list says
 *  which hooks carry one. */
function AddWebhookForm({ onCreated }: { onCreated: () => void }) {
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState('');
  const [eventType, setEventType] = useState('');
  const [saving, setSaving] = useState(false);

  if (!adding) {
    return <ListRowAddCard label="Add Webhook" onClick={() => setAdding(true)} />;
  }

  function reset() {
    setAdding(false);
    setName('');
    setEventType('');
  }

  async function save() {
    if (!name.trim() || !eventType.trim()) return;
    setSaving(true);
    try {
      const created = await createWebhook({ name: name.trim(), event_type: eventType.trim() });
      // The token exists in readable form exactly once. A toast is the only
      // place it is ever shown, so it says so and does not dismiss itself. A
      // signed hook carries no token, and this form makes unsigned ones.
      showToast(
        created.token
          ? `Token, shown only now: ${created.token}`
          : `Created ${created.name}`,
        'success',
        { autoDismissMs: created.token ? 0 : undefined },
      );
      reset();
      onCreated();
    } catch (e) {
      showToast(e instanceof Error ? e.message : 'Could not create the webhook', 'error');
    } finally {
      setSaving(false);
    }
  }

  return (
    <div class="list-row repo-add-form">
      <div class="list-row-info" style={{ gap: '0.5rem' }}>
        <input
          class="device-name-input"
          type="text"
          placeholder="Name (e.g. deploys)"
          value={name}
          onInput={(e) => setName((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => { if (e.key === 'Enter') void save(); if (e.key === 'Escape') reset(); }}
          autoFocus
        />
        <input
          class="device-name-input"
          type="text"
          placeholder="Event type (e.g. DeployFinished)"
          value={eventType}
          onInput={(e) => setEventType((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => { if (e.key === 'Enter') void save(); if (e.key === 'Escape') reset(); }}
        />
        <div class="settings-section-desc" style={{ margin: 0 }}>{EVENT_TYPE_HINT}</div>
      </div>
      <div class="list-row-actions">
        <button
          class="action-btn action-btn-confirm"
          disabled={saving || !name.trim() || !eventType.trim()}
          onClick={save}
        >
          {saving ? 'Saving...' : 'Save'}
        </button>
        <button class="action-btn" onClick={reset}>Cancel</button>
      </div>
    </div>
  );
}

/** How a webhook authenticates a delivery, in one phrase. */
export function verifierLabel(hook: Pick<Webhook, 'signed'>): string {
  return hook.signed ? 'token and signature' : 'token';
}

function WebhookRow({ hook, onChanged }: { hook: Webhook; onChanged: () => void }) {
  const [busy, setBusy] = useState(false);

  function failed(e: unknown) {
    showToast(e instanceof Error ? e.message : 'That did not work', 'error');
  }

  async function toggleEnabled() {
    setBusy(true);
    try {
      await updateWebhook(hook.id, { enabled: !hook.enabled });
      onChanged();
    } catch (e) {
      failed(e);
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    setBusy(true);
    try {
      await deleteWebhook(hook.id);
      onChanged();
    } catch (e) {
      failed(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="list-row">
      <div class="list-row-info">
        <div class="title">
          {hook.name}
          {hook.enabled ? null : ' (off)'}
        </div>
        <div class="list-row-details">
          Emits {hook.event_type}, verified by {verifierLabel(hook)}
        </div>
        <div class="list-row-details list-row-details-prose">
          <code>{hook.delivery_path}</code> on the hook port
        </div>
      </div>
      <div class="list-row-actions">
        <button class="action-btn" disabled={busy} onClick={() => void toggleEnabled()}>
          {hook.enabled ? 'Disable' : 'Enable'}
        </button>
        <button class="action-btn action-btn-danger" disabled={busy} onClick={() => void remove()}>
          Delete
        </button>
      </div>
    </div>
  );
}

export function WebhooksPage() {
  const [loadable, setLoadable] = useState<Loadable<Webhook[]>>({ status: 'not-loaded' });
  const showLoading = useDelayedLoading(loadable);

  function reload() {
    // A refetch keeps the visible list through the round-trip and swaps when
    // fresh rows land, so an SSE-driven re-read never flashes a loader.
    setLoadable(loadingIfFresh);
    fetchWebhooks()
      .then((rows) => setLoadable({ status: 'loaded', data: rows }))
      .catch((e: unknown) => setLoadable(toFailed(e)));
  }

  useEffect(reload, []);
  // A hook created, disabled or deleted from the CLI, from a chat thread or on
  // another device repaints this page with no reload (ADR 0118). Never paused:
  // the row buttons disable themselves while their own call is in flight, so
  // there is no control a reply can land under.
  useVersionedRefresh(webhooksVersion.value, false, reload);

  const hooks = loadable.status === 'loaded' ? loadable.data : [];

  return (
    <div class="settings-section">
      <p class="settings-section-desc">
        Endpoints a third party posts to. Each emits one pinned domain event, so a
        trigger can react to it. Deliveries arrive on the gateway's hook port,
        which is the only surface you can expose to the public internet.
      </p>
      {loadable.status === 'failed' && <LoadableError noun="webhooks" error={loadable.error} />}
      <div class="list-rows">
        {loadable.status === 'loaded' && hooks.length === 0 && (
          <div class="empty-state">No webhooks yet.</div>
        )}
        {loadable.status !== 'loaded' && loadable.status !== 'failed' && showLoading && (
          <div class="empty-state">Loading webhooks...</div>
        )}
        {hooks.map((hook) => (
          <WebhookRow key={hook.id} hook={hook} onChanged={reload} />
        ))}
        <AddWebhookForm onCreated={reload} />
      </div>
    </div>
  );
}
