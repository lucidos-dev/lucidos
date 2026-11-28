import { PREF_WEBHOOK_PUBLIC_URL } from '@lucidos/preference-catalog';
import { useEffect, useState } from 'preact/hooks';
import {
  createWebhook,
  deleteWebhook,
  fetchWebhooks,
  updateWebhook,
  type Webhook,
  type WebhookWithToken,
} from '../../api/client';
import type { WebhookIngressOutage, WebhookRefusal } from '../../api/client';
import { ListRowAddCard } from '../shared/ListRowAddCard';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { LoadableError } from '../shared/LoadableError';
import { credentials, preferences, showConfirm, showToast, webhooksVersion } from '../../store/store';
import { loadCredentials } from '../../store/actions/credentials';
import { loadPreferences } from '../../store/actions/preferences';
import { openCredentialSettings } from '../../store/actions/menu';
import { currentIngressOutage, ingressIsUnverified, loadWebhookIngress } from '../../store/actions/webhookIngress';
import { currentWebhookRefusals } from '../../store/actions/webhookRefusals';
import { toFailed, loadingIfFresh, type Loadable } from '../../store/types';
import { useCoarseClock } from '../../hooks/useCoarseClock';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { copyToClipboard } from '../../utils/clipboard';
import { publicWebhookDeliveryUrl, WebhookPublicUrlField } from './WebhookPublicUrlField';
import { lastDeliveryLine, lastRefusalLine } from './webhookDelivery';
import { UNVERIFIED_INGRESS_NOTICE, webhookIngressRowLine } from '../../utils/webhookIngressNotice';
import { webhookRefusalRowLine } from '../../utils/webhookRefusalNotice';
import {
  algorithmLabel,
  missingCredentialLine,
  secretReveal,
  resolveCredential,
  schemeOf,
} from './webhookSignature';
import {
  WebhookSignatureFields,
  draftBlocker,
  draftFromHmac,
  draftToHmac,
  draftToSigningSecret,
  newSignatureDraft,
  type SignatureDraft,
} from './WebhookSignatureFields';

const EVENT_TYPE_HINT =
  'PascalCase, past tense, e.g. DeployFinished. Pinned: a caller cannot change it, so one endpoint can only ever fire this event.';

export function revealSecret(result: WebhookWithToken) {
  const reveal = secretReveal(result);
  if (!reveal) {
    showToast(`Saved ${result.name}`, 'success');
    return;
  }
  showToast(`${reveal.message} ${reveal.value}`, 'success', {
    action: {
      label: reveal.copyLabel,
      onClick: () => copyToClipboard(reveal.value, 'Copied'),
    },
  });
}

function failed(e: unknown) {
  showToast(e instanceof Error ? e.message : 'That did not work', 'error');
}

function AddWebhookForm({ onCreated }: { onCreated: () => void }) {
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState('');
  const [eventType, setEventType] = useState('');
  const [signature, setSignature] = useState<SignatureDraft | null>(null);
  const [saving, setSaving] = useState(false);

  if (!adding) {
    return <ListRowAddCard label="Add Webhook" onClick={() => setAdding(true)} />;
  }

  function reset() {
    setAdding(false);
    setName('');
    setEventType('');
    setSignature(null);
  }

  const blocker = signature ? draftBlocker(signature) : null;
  const ready = !!name.trim() && !!eventType.trim() && !blocker;

  async function save() {
    if (!ready) return;
    setSaving(true);
    try {
      const hmac = signature ? draftToHmac(signature) : null;
      const created = await createWebhook({
        name: name.trim(),
        event_type: eventType.trim(),
        ...(hmac ? { hmac } : {}),
        ...(signature ? { signing_secret: draftToSigningSecret(signature) } : {}),
      });
      revealSecret(created);
      reset();
      onCreated();
    } catch (e) {
      failed(e);
    } finally {
      setSaving(false);
    }
  }

  function onFieldKey(e: KeyboardEvent) {
    if (e.key === 'Enter') void save();
    if (e.key === 'Escape') reset();
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
          onKeyDown={onFieldKey}
          autoFocus
        />
        <input
          class="device-name-input"
          type="text"
          placeholder="Event type (e.g. DeployFinished)"
          value={eventType}
          onInput={(e) => setEventType((e.target as HTMLInputElement).value)}
          onKeyDown={onFieldKey}
        />
        <div class="settings-section-desc" style={{ margin: 0 }}>{EVENT_TYPE_HINT}</div>
        {signature ? (
          <>
            <WebhookSignatureFields
              draft={signature}
              credentials={credentials.value}
              onChange={setSignature}
            />
            <button class="accent-link" onClick={() => setSignature(null)}>
              Use a token instead
            </button>
          </>
        ) : (
          <button class="accent-link" onClick={() => setSignature(newSignatureDraft(name))}>
            This sender signs its deliveries
          </button>
        )}
        {blocker && <div class="settings-section-desc" style={{ margin: 0 }}>{blocker}</div>}
      </div>
      <div class="list-row-actions">
        <button
          class="action-btn action-btn-confirm"
          disabled={saving || !ready}
          onClick={save}
        >
          {saving ? 'Saving...' : 'Save'}
        </button>
        <button class="action-btn" onClick={reset}>Cancel</button>
      </div>
    </div>
  );
}

export function verifierLabel(hook: Pick<Webhook, 'signed'>): string {
  return hook.signed ? 'signature' : 'token';
}

function SignatureRow({ hmac }: { hmac: NonNullable<Webhook['hmac']> }) {
  const link = resolveCredential(hmac, credentials.value);

  if (link.state === 'missing') {
    return (
      <div class="list-row-details list-row-details-prose webhook-credential-missing">
        {missingCredentialLine(link.name)}
      </div>
    );
  }
  return (
    <div class="list-row-details">
      <span>
        Signed with{' '}
        {link.state === 'found' ? (
          <button class="accent-link" onClick={() => openCredentialSettings(link.id)}>
            {link.name}
          </button>
        ) : (
          link.name
        )}
      </span>
      <span>{hmac.signature_header}</span>
      <span>{algorithmLabel(hmac)}</span>
    </div>
  );
}

export function confirmWebhookDeletion(hook: Pick<Webhook, 'name'>): Promise<boolean> {
  return showConfirm(
    `Delete the webhook "${hook.name}"?\n\n` +
      'Its endpoint stops answering, so any sender still posting to it starts ' +
      'failing. A new webhook gets a different path, so this one cannot be restored.',
    'Delete',
    { variant: 'danger' },
  );
}

function WebhookRow(
  { hook, publicUrl = '', unverified = false, outage = null, refusal = null, now = new Date(0), onChanged = () => {} }: {
    hook?: Webhook;
    publicUrl?: string;
    unverified?: boolean;
    outage?: WebhookIngressOutage | null;
    refusal?: WebhookRefusal | null;
    now?: Date;
    onChanged?: () => void;
  },
) {
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<SignatureDraft | null>(null);
  const deliveryUrl = hook && publicWebhookDeliveryUrl(publicUrl, hook.delivery_path);
  const delivery = hook ? lastDeliveryLine(hook, now) : null;
  const lastRefusal = hook ? lastRefusalLine(hook, now) : null;

  async function change(fn: () => Promise<unknown>) {
    setBusy(true);
    try {
      await fn();
      onChanged();
    } catch (e) {
      failed(e);
    } finally {
      setBusy(false);
    }
  }

  async function saveSignature() {
    if (!hook || !editing) return;
    const hmac = draftToHmac(editing);
    if (!hmac) return;
    if (!hook.signed) {
      const go = await showConfirm(
        `${hook.name} currently authenticates with a bearer token. Signing it ` +
          'stops that token working, and it cannot be recovered.\n\n' +
          'Anything presenting it has to move to the signature.',
        'Sign it',
        { variant: 'danger' },
      );
      if (!go) return;
    }
    await change(async () => {
      revealSecret(
        await updateWebhook(hook.id, {
          hmac,
          signing_secret: draftToSigningSecret(editing),
        }),
      );
      setEditing(null);
    });
  }

  async function removeSignature() {
    if (!hook) return;
    await change(async () => {
      revealSecret(await updateWebhook(hook.id, { hmac: null }));
      setEditing(null);
    });
  }

  async function deleteHook() {
    if (!hook || !(await confirmWebhookDeletion(hook))) return;
    await change(async () => {
      await deleteWebhook(hook.id);
    });
  }

  const blocker = editing ? draftBlocker(editing) : null;

  return (
    <div class="list-row">
      <div class="list-row-info">
        <SkText class="title" as="div" w="9rem">
          {hook?.name}
          {hook?.enabled === false ? ' (off)' : null}
        </SkText>
        <SkText class="list-row-details" as="div" w="16rem">
          {hook && <>Emits {hook.event_type}, verified by {verifierLabel(hook)}</>}
        </SkText>
        {hook?.hmac && !editing && <SignatureRow hmac={hook.hmac} />}
        <SkText class="list-row-details list-row-details-prose webhook-delivery-url" as="div" w="13rem">
          {hook && <><code>{deliveryUrl ?? hook.delivery_path}</code>{!deliveryUrl && ' on the hook port'}</>}
        </SkText>
        {delivery && <div class="list-row-details">{delivery}</div>}
        {lastRefusal && <div class="list-row-details">{lastRefusal}</div>}
        {refusal && (
          <div class="list-row-details webhook-refusal-warning">
            {webhookRefusalRowLine(refusal)}
          </div>
        )}
        {hook?.enabled && unverified && (
          <div class="list-row-details">{UNVERIFIED_INGRESS_NOTICE}</div>
        )}
        {hook?.enabled && outage && (
          <div class="list-row-details webhook-ingress-warning">
            {webhookIngressRowLine(outage)}
          </div>
        )}
        {editing && (
          <>
            <WebhookSignatureFields
              draft={editing}
              credentials={credentials.value}
              onChange={setEditing}
            />
            {blocker && <div class="settings-section-desc" style={{ margin: 0 }}>{blocker}</div>}
            <div class="list-row-actions">
              <button
                class="action-btn action-btn-confirm"
                disabled={busy || !!blocker}
                onClick={() => void saveSignature()}
              >
                Save signature
              </button>
              {hook?.hmac && (
                <button
                  class="action-btn action-btn-danger"
                  disabled={busy}
                  onClick={() => void removeSignature()}
                >
                  Remove signature
                </button>
              )}
              <button class="action-btn" onClick={() => setEditing(null)}>Cancel</button>
            </div>
          </>
        )}
      </div>
      <div class="list-row-actions">
        {hook && !editing && (
          <button
            class="action-btn"
            disabled={busy}
            onClick={() => setEditing(
              hook.hmac
                ? draftFromHmac(hook.hmac, schemeOf(hook.hmac))
                : newSignatureDraft(hook.name),
            )}
          >
            Signature
          </button>
        )}
        <SkBlock w="4.5rem" h="2rem" round>
          <button
            class="action-btn"
            disabled={busy}
            onClick={() => hook && void change(async () => {
              await updateWebhook(hook.id, { enabled: !hook.enabled });
            })}
          >
            {hook?.enabled ? 'Disable' : 'Enable'}
          </button>
        </SkBlock>
        <SkBlock w="4rem" h="2rem" round>
          <button
            class="action-btn action-btn-danger"
            disabled={busy}
            onClick={() => void deleteHook()}
          >
            Delete
          </button>
        </SkBlock>
      </div>
    </div>
  );
}

export function WebhooksPage() {
  const [loadable, setLoadable] = useState<Loadable<Webhook[]>>({ status: 'not-loaded' });
  const showLoading = useDelayedLoading(loadable);

  function reload(): Promise<void> {
    setLoadable(loadingIfFresh);
    return fetchWebhooks()
      .then((rows) => setLoadable({ status: 'loaded', data: rows }))
      .catch((e: unknown) => setLoadable(toFailed(e)));
  }

  useEffect(() => {
    void reload();
    void loadPreferences();
  }, []);
  usePanelRefresh('webhooks', () => Promise.all([
    reload(),
    loadPreferences(),
    loadWebhookIngress(),
  ]));
  useEffect(() => {
    if (credentials.value.status === 'not-loaded') void loadCredentials();
  }, []);
  useVersionedRefresh(webhooksVersion.value, false, reload);

  const hooks = loadable.status === 'loaded' ? loadable.data : [];
  const tick = useCoarseClock();
  const now = new Date(tick);
  const outage = currentIngressOutage(tick);
  const unverified = ingressIsUnverified();
  const prefs = preferences.value;
  const publicUrl = prefs.status === 'loaded' ? prefs.data[PREF_WEBHOOK_PUBLIC_URL.key] ?? '' : '';
  const refusals = new Map(
    currentWebhookRefusals(tick).map((refusal) => [refusal.webhook_id, refusal]),
  );

  return (
    <div class="settings-section">
      <p class="settings-section-desc">
        Endpoints a third party posts to. Each emits one pinned domain event, so a
        trigger can react to it. Deliveries arrive on the gateway's hook port,
        which is the only surface you can expose to the public internet.
      </p>
      {prefs.status === 'loaded' && <WebhookPublicUrlField stored={publicUrl} />}
      {prefs.status === 'failed' && <LoadableError noun="preferences" error={prefs.error} />}
      {loadable.status === 'failed' && <LoadableError noun="webhooks" error={loadable.error} />}
      <LoadingFade
        showSkeleton={showLoading}
        skeleton={<ListSkeletonOf count={2} containerClass="list-rows" row={() => <WebhookRow />} />}
      >
        {loadable.status === 'loaded' && (
          <div class="list-rows">
            {hooks.length === 0 && <div class="empty-state">No webhooks yet.</div>}
            {hooks.map((hook) => (
              <WebhookRow
                key={hook.id}
                hook={hook}
                publicUrl={publicUrl}
                unverified={unverified}
                outage={outage}
                refusal={refusals.get(hook.id) ?? null}
                now={now}
                onChanged={reload}
              />
            ))}
          </div>
        )}
      </LoadingFade>
      <div class="list-rows">
        <AddWebhookForm onCreated={reload} />
      </div>
    </div>
  );
}
