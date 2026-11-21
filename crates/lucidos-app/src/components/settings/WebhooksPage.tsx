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
import { credentials, showConfirm, showToast, webhooksVersion } from '../../store/store';
import { loadCredentials } from '../../store/actions/credentials';
import { openCredentialSettings } from '../../store/actions/menu';
import { currentIngressOutage } from '../../store/actions/webhookIngress';
import { currentWebhookRefusals } from '../../store/actions/webhookRefusals';
import { toFailed, loadingIfFresh, type Loadable } from '../../store/types';
import { useCoarseClock } from '../../hooks/useCoarseClock';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { copyToClipboard } from '../../utils/clipboard';
import { lastDeliveryLine, lastRefusalLine } from './webhookDelivery';
import { webhookIngressRowLine } from '../../utils/webhookIngressNotice';
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

/** The event a webhook emits, as the user must type it. */
const EVENT_TYPE_HINT =
  'PascalCase, past tense, e.g. DeployFinished. Pinned: a caller cannot change it, so one endpoint can only ever fire this event.';

/** Show whatever secret a response handed back, if it handed one back.
 *
 *  It never dismisses itself, and a tap on it copies the token (its lone action
 *  is the card tap). A bearer token is the one the user cannot get again: only
 *  its digest is stored. */
export function revealSecret(result: WebhookWithToken) {
  const reveal = secretReveal(result);
  if (!reveal) {
    showToast(`Saved ${result.name}`, 'success');
    return;
  }
  // The Copy action is what keeps it up: an action toast waits to be answered.
  // A delay of 0 would mean "leave at once", not "never".
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

/** The form for a new webhook.
 *
 *  Signing is optional and off until asked for, so the common hook is still
 *  two fields. Asking for it opens the editor an existing row also uses. */
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

  /** Enter saves from the two plain fields only. Inside the signature section
   *  it would submit a half-filled config from under a dropdown. */
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
          // Seeds a credential name from the hook's own name, so the common
          // case is one fewer field to fill in.
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

/** How a webhook authenticates a delivery, in one phrase.
 *
 *  A signed hook has no token. `create` mints one only for an unsigned hook.
 *  An update that adds a signature drops any token the hook had, because
 *  `verify` requires every verifier a row carries. */
export function verifierLabel(hook: Pick<Webhook, 'signed'>): string {
  return hook.signed ? 'signature' : 'token';
}

/** What a signed hook verifies with, as one compact row of fields.
 *
 *  The credential links into Settings > Accounts, scrolled to its own row. A
 *  name the store cannot resolve links nowhere and says so. That state is
 *  `DeliveryRefusal::CredentialMissing`, and it is the failure a user most
 *  needs to see: the hook refuses every delivery until it is fixed. */
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
      {/* One flex item, not three: `.list-row-details` blockifies every child,
          so an unwrapped link would strand the word before it. */}
      <span>
        Signed with{' '}
        {/* Not a link until the credentials are loaded, since only then is
            there an id to land on. The name still reads, so the row does not
            reflow when the list arrives. */}
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

/** Ask before the endpoint goes.
 *
 *  The delivery path carries the hook's id, so a replacement is a different
 *  URL rather than this one restored. Every sender then needs repointing,
 *  which is work outside this workspace. */
export function confirmWebhookDeletion(hook: Pick<Webhook, 'name'>): Promise<boolean> {
  return showConfirm(
    `Delete the webhook "${hook.name}"?\n\n` +
      'Its endpoint stops answering, so any sender still posting to it starts ' +
      'failing. A new webhook gets a different path, so this one cannot be restored.',
    'Delete',
    { variant: 'danger' },
  );
}

/** One webhook. With no `hook`, inside a `SkeletonProvider`, it draws itself
 *  as the loading placeholder. */
function WebhookRow(
  { hook, outage = null, refusal = null, now = new Date(0), onChanged = () => {} }: {
    hook?: Webhook;
    outage?: WebhookIngressOutage | null;
    /** This hook's own standing refusal, or null. Unlike `outage`, which is
     *  the shared path and is drawn on every enabled row. */
    refusal?: WebhookRefusal | null;
    now?: Date;
    onChanged?: () => void;
  },
) {
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<SignatureDraft | null>(null);
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
    // Signing an unsigned hook drops its bearer token, because a hook carries
    // exactly one verifier. Only the digest was stored, so whatever presents
    // that token stops working and it cannot be read back. Say so first.
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

  /** Turning a signed hook unsigned mints a bearer token, shown once. A hook
   *  always carries exactly one verifier, so it cannot just lose this one. */
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
        <SkText class="list-row-details list-row-details-prose" as="div" w="13rem">
          {hook && <><code>{hook.delivery_path}</code> on the hook port</>}
        </SkText>
        {delivery && <div class="list-row-details">{delivery}</div>}
        {lastRefusal && <div class="list-row-details">{lastRefusal}</div>}
        {/* The engine's own verdict on this hook, which the line above cannot
            give: one refusal is a fact, and a run of them is an outage. */}
        {refusal && (
          <div class="list-row-details webhook-refusal-warning">
            {webhookRefusalRowLine(refusal)}
          </div>
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
        {/* Reopens what the hook stores, or starts a fresh draft for an
            unsigned one. */}
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
    // A refetch keeps the visible list through the round-trip and swaps when
    // fresh rows land, so an SSE-driven re-read never flashes a loader.
    setLoadable(loadingIfFresh);
    return fetchWebhooks()
      .then((rows) => setLoadable({ status: 'loaded', data: rows }))
      .catch((e: unknown) => setLoadable(toFailed(e)));
  }

  useEffect(() => { void reload(); }, []);
  usePanelRefresh('webhooks', reload);
  // A signed hook names a credential, and this page says whether that
  // credential still exists. The signal is kept fresh by the `Credential*` SSE
  // arm, so reading it IS the subscription. This only covers a cold open.
  useEffect(() => {
    if (credentials.value.status === 'not-loaded') void loadCredentials();
  }, []);
  // A hook created, disabled or deleted from the CLI, from a chat thread or on
  // another device repaints this page with no reload (ADR 0118). Never paused:
  // the row buttons disable themselves while their own call is in flight, so
  // there is no control a reply can land under.
  useVersionedRefresh(webhooksVersion.value, false, reload);

  const hooks = loadable.status === 'loaded' ? loadable.data : [];
  // One clock for the whole page. Read per row, two rows drawn either side of a
  // second disagreed, and every elapsed-time label froze after the first paint.
  const tick = useCoarseClock();
  const now = new Date(tick);
  // The standing outage of the path every hook shares, read through the same
  // selector the app bar reads. Drawn on every enabled row rather than on the
  // hook the probe happened to target: what failed sits in front of all of them.
  const outage = currentIngressOutage(tick);
  // Per hook, unlike the outage above: the engine judges each hook's own
  // record, so fixing one says nothing about another.
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
                outage={outage}
                refusal={refusals.get(hook.id) ?? null}
                now={now}
                onChanged={reload}
              />
            ))}
          </div>
        )}
      </LoadingFade>
      {/* Outside the fade, so a reload never unmounts a half-typed webhook. */}
      <div class="list-rows">
        <AddWebhookForm onCreated={reload} />
      </div>
    </div>
  );
}
