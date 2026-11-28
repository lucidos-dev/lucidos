
import { loadPairedRemoteGateways, pairedRemoteGateways } from '../../store/actions/settingsGateway';
import { headersFromDraft, type HeaderDraft, RemoteHeadersEditor } from './RemoteHeadersEditor';
import { useCallback, useEffect, useState } from 'preact/hooks';
import {
  listPairedGateways,
  listRemoteWorkspaces,
  mintGatewayPairingCode,
  removeRemoteGateway,
  repairRemoteGateway,
  updateRemoteGatewayHeaders,
  rotateRemoteGateway,
  setPairedGatewayGrant,
  setPairedGatewayRemoteStarts,
  setRemoteGatewayReturnAccess,
  setRemoteGatewayTrustedPairing,
  unpairGateway,
  type HopGrant,
  type PairedGateway,
  type RemoteGateway,
  type RemoteGatewayListing,
  type RemoteStartSetting,
} from '../../api/client/control';
import { untilOwnerAllows } from '../../api/client/awaitingOwner';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { showConfirm, showToast } from '../../store/store';
import { loadingIfFresh, toFailed, type Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { Disclosure } from '../shared/Disclosure';
import { DisclosureChevron } from '../shared/DisclosureChevron';
import { Dropdown, type DropdownOption } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { CopyButton, PairCard, expiryLabel, type PairTarget } from './PairDeviceSection';
import { timeAgo } from './pairedDevices';
import {
  AWAITING_OWNER_NOTE,
  EMPTY_REMOTE_GATEWAY_DRAFT,
  HTTPS_ONLY_NOTE,
  TRUST_NOTE,
  remoteGatewayBlocker,
  shortNameInput,
  submitRemoteGateway,
  type RemoteGatewayDraft,
} from './remoteGatewayForm';
import { useCodeCountdown } from './useCodeCountdown';

export function gatewayPairingAddress(target: PairTarget): string | null {
  return target.kind === 'origin' && target.url.startsWith('https://') ? target.url : null;
}

function useGatewayRead<T>(
  available: boolean,
  read: () => Promise<T[]>,
): [Loadable<T[]>, () => Promise<unknown>] {
  const [list, setList] = useState<Loadable<T[]>>({ status: 'not-loaded' });
  const reload = useCallback((): Promise<unknown> => {
    if (!available) return Promise.resolve();
    setList(loadingIfFresh);
    return read().then(
      (data) => setList({ status: 'loaded', data }),
      (e) => setList(toFailed(e)),
    );
  }, [available, read]);
  useEffect(() => { void reload(); }, [reload]);
  return [list, reload];
}

function useGatewayList<T>(
  available: boolean,
  read: () => Promise<T[]>,
  name: string,
): [Loadable<T[]>, () => Promise<unknown>] {
  const [list, reload] = useGatewayRead(available, read);
  usePanelRefresh(name, reload);
  return [list, reload];
}

function GatewayRow({
  title,
  detail,
  action,
}: {
  title?: preact.ComponentChildren;
  detail?: preact.ComponentChildren;
  action?: preact.ComponentChildren;
}) {
  return (
    <div class="list-row">
      <div class="list-row-info">
        <SkText class="title" as="div" w="9rem">{title}</SkText>
        <SkText class="list-row-details list-row-details-prose" as="div" w="14rem">
          {detail}
        </SkText>
      </div>
      <div class="list-row-actions">
        <SkBlock w="4.5rem" h="2rem" round>{action}</SkBlock>
      </div>
    </div>
  );
}

function GatewayList<T>({
  list,
  noun,
  empty,
  row,
}: {
  list: Loadable<T[]>;
  noun: string;
  empty?: string;
  row: (item: T) => preact.VNode;
}) {
  const showLoading = useDelayedLoading(list);
  if (list.status === 'failed') return <LoadableError noun={noun} error={list.error} />;
  return (
    <LoadingFade
      showSkeleton={showLoading}
      skeleton={<ListSkeletonOf row={() => <GatewayRow />} count={1} />}
    >
      {list.status === 'loaded' && (
        list.data.length === 0
          ? empty && <div class="settings-section-desc">{empty}</div>
          : list.data.map(row)
      )}
    </LoadingFade>
  );
}

export const REMOTE_START_OPTIONS: (DropdownOption & { value: RemoteStartSetting })[] = [
  { value: 'allow', label: 'Starts allowed' },
  { value: 'ask', label: 'Starts ask me' },
  { value: 'deny', label: 'Starts refused' },
];

export function remoteStartsDetail(setting: RemoteStartSetting): string {
  switch (setting) {
    case 'allow': return 'its agents may start threads here';
    case 'ask': return 'its agents ask you before a thread starts here';
    case 'deny': return 'its agents may not start threads here';
  }
}

const HOP_GRANTS: readonly HopGrant[] = ['manage', 'control', 'admin'];

const GRANT_SCOPES: Record<HopGrant, string> = {
  manage: 'manage users and devices',
  control: 'create, start, stop and restore workspaces',
  admin: 'manage this gateway as a device paired here',
};

const GRANT_CONFIRMS: Record<HopGrant, (label: string) => { title: string; message: string }> = {
  manage: (label) => ({
    title: `Let ${label} manage users and devices?`,
    message: `Every device paired with ${label} can then list, revoke and reassign this gateway's `
      + 'devices, and add, rename and remove its users.\n\n'
      + 'Pairing codes, gateway pairings and the rest of Settings stay on this gateway.',
  }),
  control: (label) => ({
    title: `Let ${label} create and restore workspaces?`,
    message: `Every device paired with ${label} can then create, start and stop this gateway's `
      + 'workspaces, and restore a backup into a new one. That uses disk and memory here.\n\n'
      + 'Deleting, renaming and the rest of Settings stay on this gateway.',
  }),
  admin: (label) => ({
    title: `Let ${label} manage this gateway?`,
    message: `Every device paired with ${label} can then use this gateway's Settings Gateway group `
      + 'as a device paired here would: access, updates, devices, users, workspaces and gateway '
      + 'pairing. It includes both grants above.\n\n'
      + 'Only a device paired with this gateway can turn a grant on or off.',
  }),
};

const GRANT_DETAILS: Record<HopGrant, string> = {
  manage: 'may manage its users and devices',
  control: 'may create and restore its workspaces',
  admin: 'may manage it as a device paired here',
};

export function impliedByAdmin(grant: HopGrant, admin: boolean): boolean {
  return admin && grant !== 'admin';
}

export function grantsDetail(gateway: PairedGateway): string {
  const shown = gateway.may_admin ? (['admin'] as const) : HOP_GRANTS;
  const granted = shown.filter((grant) => gateway[`may_${grant}`]).map((grant) => GRANT_DETAILS[grant]);
  return ["can open this gateway's workspaces", ...granted].join(' · ');
}

function GrantSwitch({ label, on, locked, onChange }: {
  label: string;
  on: boolean;
  locked: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <label class="toggle-switch" data-tooltip={locked ? `${label} (included in admin)` : label}>
      <input
        type="checkbox"
        aria-label={label}
        checked={on || locked}
        disabled={locked}
        onChange={(e) => {
          const input = e.currentTarget as HTMLInputElement;
          const wanted = input.checked;
          input.checked = on;
          onChange(wanted);
        }}
      />
      <span class="toggle-slider" />
    </label>
  );
}

function GrantToggle({ gateway, grant, onChange }: {
  gateway: PairedGateway;
  grant: HopGrant;
  onChange: (on: boolean) => void;
}) {
  return (
    <GrantSwitch
      label={`Let devices on ${gateway.label} ${GRANT_SCOPES[grant]}`}
      on={gateway[`may_${grant}`]}
      locked={impliedByAdmin(grant, gateway.may_admin)}
      onChange={onChange}
    />
  );
}

function MintGrantTicks({ ticked, onChange }: {
  ticked: ReadonlySet<HopGrant>;
  onChange: (grant: HopGrant, on: boolean) => void;
}) {
  return (
    <>
      {HOP_GRANTS.map((grant) => {
        const label = `Let devices on the other gateway ${GRANT_SCOPES[grant]}`;
        return (
          <div class="list-row" key={grant}>
            <div class="list-row-info">
              <div class="list-row-details list-row-details-prose">{label}</div>
            </div>
            <div class="list-row-actions">
              <GrantSwitch
                label={label}
                on={ticked.has(grant)}
                locked={impliedByAdmin(grant, ticked.has('admin'))}
                onChange={(on) => onChange(grant, on)}
              />
            </div>
          </div>
        );
      })}
    </>
  );
}

interface MintedGatewayCode {
  code: string;
  expiresAt: number;
}

export function PairGatewaySection({
  available,
  target,
}: {
  available: boolean;
  target: PairTarget;
}) {
  const [minted, setMinted] = useState<Loadable<MintedGatewayCode>>({ status: 'not-loaded' });
  const [ticked, setTicked] = useState<ReadonlySet<HopGrant>>(new Set());
  const [paired, reload] = useGatewayList<PairedGateway>(
    available,
    listPairedGateways,
    'pair a gateway',
  );
  const left = useCodeCountdown(minted.status === 'loaded' ? minted.data.expiresAt : null);
  const address = gatewayPairingAddress(target);

  const mint = () => {
    setMinted({ status: 'loading' });
    mintGatewayPairingCode([...ticked]).then(
      (body) => setMinted({
        status: 'loaded',
        data: { code: body.code, expiresAt: Date.now() + body.expires_in_secs * 1000 },
      }),
      (e) => setMinted(toFailed(e)),
    );
  };

  const unpair = async (gateway: PairedGateway) => {
    const ok = await showConfirm(
      `${gateway.label} will no longer open this gateway's workspaces. Pairing again takes a new gateway secret.`,
      'Unpair',
      { variant: 'danger' },
    );
    if (!ok) return;
    try {
      await unpairGateway(gateway.id);
      await reload();
    } catch (e) {
      showToast(`Couldn't unpair ${gateway.label}: ${errorDetail(e)}`, 'error');
    }
  };

  const tick = (grant: HopGrant, on: boolean) => {
    const next = new Set(ticked);
    if (on) next.add(grant);
    else next.delete(grant);
    setTicked(next);
  };

  const setGrant = async (gateway: PairedGateway, grant: HopGrant, on: boolean) => {
    if (on) {
      const ask = GRANT_CONFIRMS[grant](gateway.label);
      const ok = await showConfirm(ask.message, 'Allow', { title: ask.title, variant: 'default' });
      if (!ok) return;
    }
    try {
      await setPairedGatewayGrant(gateway.id, grant, on);
    } catch (e) {
      showToast(`Couldn't change what ${gateway.label} may do here: ${errorDetail(e)}`, 'error');
    }
    await reload();
  };

  const setRemoteStarts = async (gateway: PairedGateway, setting: RemoteStartSetting) => {
    try {
      await setPairedGatewayRemoteStarts(gateway.id, setting);
    } catch (e) {
      showToast(`Couldn't change remote starts from ${gateway.label}: ${errorDetail(e)}`, 'error');
    }
    await reload();
  };

  function codeBody() {
    if (minted.status === 'failed') {
      return (
        <div class="list-row">
          <div class="list-row-info">
            <LoadableError noun="a gateway pairing secret" error={minted.error} />
          </div>
          <div class="list-row-actions">
            <button class="action-btn" onClick={mint}>Try again</button>
          </div>
        </div>
      );
    }
    if (minted.status !== 'loaded') {
      return (
        <>
        <MintGrantTicks ticked={ticked} onChange={tick} />
        <div class="list-row">
          <div class="list-row-info">
            <div class="title">Let another gateway in</div>
            <div class="list-row-details list-row-details-prose">
              {paired.status === 'loaded' && paired.data.length === 0
                ? "No other gateway can open this gateway's workspaces yet."
                : 'Creates a one-time secret for the other gateway to paste.'}
            </div>
          </div>
          <div class="list-row-actions">
            <button
              class="action-btn action-btn-confirm"
              disabled={minted.status === 'loading'}
              onClick={mint}
            >
              {minted.status === 'loading' ? 'Creating…' : 'Create a gateway secret'}
            </button>
          </div>
        </div>
        </>
      );
    }
    const expired = left <= 0;
    return (
      <div class="add-device">
        <div class="add-device-status">
          <span class="add-device-expiry">{expiryLabel(left)}</span>
          <button class="action-btn" onClick={mint}>{expired ? 'Create another' : 'New secret'}</button>
        </div>
        {!expired && (
          <div class="add-device-cards">
            <div class="add-device-alts">
              <PairCard
                kind="code"
                label="Paste this secret on the other gateway"
                action={<CopyButton text={minted.data.code} what="gateway pairing secret" />}
              >
                <div class="add-device-code add-device-secret">{minted.data.code}</div>
              </PairCard>
              {address ? (
                <PairCard
                  kind="url"
                  label="With this address"
                  action={<CopyButton text={address} what="gateway address" />}
                >
                  <div class="add-device-url">{address}</div>
                </PairCard>
              ) : (
                <div class="add-device-note">{HTTPS_ONLY_NOTE}</div>
              )}
            </div>
          </div>
        )}
      </div>
    );
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="access:pair-gateway">
        Pair a gateway
        <Explainer title="Pair a gateway">
          <p>
            Lets another Lucidos gateway open this gateway's workspaces, for every device
            paired with it. The other gateway pastes the secret under Remote gateways, and
            needs this version of Lucidos or newer.
          </p>
          <p>
            It is one-way: this gateway gains nothing on the other one. Unpair ends it on
            the other gateway's next request.
          </p>
          <p>
            The switches above the secret choose what devices on the other gateway may do
            here. Admin includes the other two. Only a device paired with this gateway can
            change them later.
          </p>
          <p>
            Each pairing also sets whether its agents may start threads here. Ask parks
            each new thread on a card until you allow it.
          </p>
        </Explainer>
      </div>
      {available ? (
        <div class="list-rows">
          {codeBody()}
          <GatewayList
            list={paired}
            noun="paired gateways"
            row={(gateway) => (
              <GatewayRow
                key={gateway.id}
                title={gateway.label}
                detail={`Paired ${timeAgo(gateway.paired_at, Date.now())} · ${gateway.last_used_at ? `last used ${timeAgo(gateway.last_used_at, Date.now())}` : 'not used yet'} · ${grantsDetail(gateway)} · ${remoteStartsDetail(gateway.remote_starts)}`}
                action={
                  <>
                    <Dropdown
                      options={REMOTE_START_OPTIONS}
                      value={gateway.remote_starts}
                      onChange={(value) => void setRemoteStarts(gateway, value as RemoteStartSetting)}
                    />
                    {HOP_GRANTS.map((grant) => (
                      <GrantToggle
                        key={grant}
                        gateway={gateway}
                        grant={grant}
                        onChange={(on) => void setGrant(gateway, grant, on)}
                      />
                    ))}
                    <button class="action-btn action-btn-danger" onClick={() => void unpair(gateway)}>
                      Unpair
                    </button>
                  </>
                }
              />
            )}
          />
        </div>
      ) : (
        <div class="settings-section-desc">
          This page is served straight off the engine, so it cannot reach the pairing
          service. Run <code>lucidos pair --gateway</code> in a terminal on the machine
          running Lucidos.
        </div>
      )}
    </div>
  );
}

export type RemoteGatewayState =
  | { kind: 'unknown' }
  | { kind: 'reachable' }
  | { kind: 'revoked' }
  | { kind: 'unreachable'; reason: string };

export function remoteGatewayState(
  listings: Loadable<RemoteGatewayListing[]>,
  gateway: RemoteGateway,
): RemoteGatewayState {
  if (listings.status !== 'loaded') return { kind: 'unknown' };
  const listing = listings.data.find((l) =>
    l.gateway === gateway.name && (l.pairing_id === undefined || l.pairing_id === gateway.pairing_id));
  if (!listing) return { kind: 'unknown' };
  if (listing.state === 'reachable') return { kind: 'reachable' };
  return listing.reason_kind === 'revoked' ? { kind: 'revoked' } : { kind: 'unreachable', reason: listing.reason };
}

const STATE_WORDS: Record<'revoked' | 'unreachable', string> = {
  revoked: 'Access revoked',
  unreachable: 'Not reachable',
};

function RemoteGatewayDetail({ gateway, state }: { gateway: RemoteGateway; state: RemoteGatewayState }) {
  const added = `${gateway.address} · added ${timeAgo(gateway.added_at, Date.now())}`;
  if (state.kind !== 'revoked' && state.kind !== 'unreachable') return <>{added}</>;
  return (
    <>
      <span class={`remote-gateway-state remote-gateway-state-${state.kind}`}>{STATE_WORDS[state.kind]}</span>
      {` · ${added}`}
    </>
  );
}

function RepairRemoteGateway({ gateway, reload }: { gateway: RemoteGateway; reload: () => Promise<unknown> }) {
  const [code, setCode] = useState('');
  const [pairing, setPairing] = useState(false);
  const [awaitingOwner, setAwaitingOwner] = useState(false);
  const label = `New gateway pairing secret from ${gateway.name}`;
  const repair = async () => {
    if (!code.trim() || pairing) return;
    setPairing(true);
    try {
      const renewed = await untilOwnerAllows(
        () => repairRemoteGateway(gateway.name, code.trim()),
        () => setAwaitingOwner(true),
      );
      setCode('');
      showToast(renewed.return_access_warning ?? `Paired again with ${gateway.name}`,
        renewed.return_access_warning ? 'warning' : 'success');
      await reload();
    } catch (e) {
      showToast(`Couldn't pair again with ${gateway.name}: ${errorDetail(e)}`, 'error');
    } finally {
      setPairing(false);
      setAwaitingOwner(false);
    }
  };
  return (
    <div>
      <div class="settings-row-note" role="status">
        {gateway.name} unpaired this gateway, so its workspaces no longer open here. On {gateway.name},
        open Settings → Access → Pair a gateway and paste the new secret here. The short name, headers
        and agent return access stay.
      </div>
      <div class="settings-row update-address-row">
        <div class="update-address-controls">
          <input
            class="settings-text-input update-address-input"
            value={code}
            placeholder="lgp_…"
            aria-label={label}
            spellcheck={false}
            disabled={pairing}
            onInput={(e) => setCode((e.currentTarget as HTMLInputElement).value)}
            onKeyDown={(e) => { if (e.key === 'Enter') void repair(); }}
          />
          <button class="action-btn action-btn-confirm" disabled={!code.trim() || pairing} onClick={() => void repair()}>
            {awaitingOwner ? 'Waiting…' : pairing ? 'Pairing…' : 'Re-pair'}
          </button>
        </div>
      </div>
      <Disclosure open={awaitingOwner}>
        <div class="settings-row-note" role="status">{AWAITING_OWNER_NOTE}</div>
      </Disclosure>
    </div>
  );
}

interface HeadersDraft {
  editing: boolean;
  rows: HeaderDraft[];
}

const CLOSED_HEADERS_DRAFT: HeadersDraft = { editing: false, rows: [] };

function RemoteGatewaySettings({
  gateway,
  reload,
  headers,
  setHeaders,
}: {
  gateway: RemoteGateway;
  reload: () => Promise<unknown>;
  headers: HeadersDraft;
  setHeaders: (draft: HeadersDraft) => void;
}) {
  const { editing, rows } = headers;
  const [saving, setSaving] = useState(false);
  const returnLabel = `Allow agents on ${gateway.name} to read and start threads through this gateway`;
  const setReturnAccess = async (enabled: boolean) => {
    setSaving(true);
    try {
      const updated = await setRemoteGatewayReturnAccess(gateway.name, enabled);
      showToast(updated.return_access_warning ?? `Agent return access ${enabled ? 'enabled' : 'disabled'} for ${gateway.name}`,
        updated.return_access_warning ? 'warning' : 'success');
    } catch (e) {
      showToast(`Couldn't change agent return access for ${gateway.name}: ${errorDetail(e)}`, 'error');
    } finally {
      await reload();
      setSaving(false);
    }
  };
  const trustedLabel = `Use ${gateway.name}'s own app for its workspaces`;
  const setTrustedPairing = async (enabled: boolean) => {
    if (enabled && !await showConfirm(
      `${gateway.name}'s app will run here with this gateway's access, on every device that opens its workspaces through this gateway.\n\nTurn this on only if you own ${gateway.name}.`,
      'Trust', { variant: 'default' })) return;
    setSaving(true);
    try {
      await setRemoteGatewayTrustedPairing(gateway.name, enabled);
      showToast(`${gateway.name}'s own app ${enabled ? 'enabled' : 'disabled'}`, 'success');
    } catch (e) {
      showToast(`Couldn't change trusted pairing for ${gateway.name}: ${errorDetail(e)}`, 'error');
    } finally {
      await reload();
      setSaving(false);
    }
  };
  const rotate = async () => {
    setSaving(true);
    try {
      await rotateRemoteGateway(gateway.name);
      showToast(`Credential rotated for ${gateway.name}`, 'success');
    } catch (e) {
      showToast(`Couldn't rotate credential for ${gateway.name}: ${errorDetail(e)}`, 'error');
    } finally {
      await reload();
      setSaving(false);
    }
  };
  const save = async () => {
    setSaving(true);
    try {
      await updateRemoteGatewayHeaders(gateway.name, headersFromDraft(rows));
      setHeaders(CLOSED_HEADERS_DRAFT);
      await reload();
    } catch (e) {
      showToast(`Couldn't update headers for ${gateway.name}: ${errorDetail(e)}`, 'error');
    } finally { setSaving(false); }
  };
  return <>
    <div class="settings-row">
      <span class="settings-row-label">
        Agent return access
        <Explainer title="Agent return access">
          On: agents on {gateway.name} may read and start threads through this gateway. Off: they cannot reach
          back here at all.
        </Explainer>
      </span>
      <label class="toggle-switch" data-tooltip={returnLabel}>
        <input type="checkbox" aria-label={returnLabel} checked={gateway.return_access ?? false}
          disabled={saving || gateway.return_access === undefined}
          onChange={(e) => {
            const input = e.currentTarget as HTMLInputElement;
            const wanted = input.checked;
            input.checked = gateway.return_access ?? false;
            void setReturnAccess(wanted);
          }} />
        <span class="toggle-slider" />
      </label>
    </div>
    {gateway.return_access === undefined && <div class="settings-row-note">Update this gateway to manage agent return access.</div>}
    <div class="settings-row">
      <span class="settings-row-label">
        Use its own app
        <Explainer title="Trusted pairing">
          Off: this gateway's app opens {gateway.name}'s workspaces. On: {gateway.name} serves its own app, so you
          get its version and features. Its code then runs with this gateway's access. Turn it on only for a gateway you own.
        </Explainer>
      </span>
      <label class="toggle-switch" data-tooltip={trustedLabel}>
        <input type="checkbox" aria-label={trustedLabel} checked={gateway.trusted_pairing ?? false}
          disabled={saving || gateway.trusted_pairing === undefined}
          onChange={(e) => {
            const input = e.currentTarget as HTMLInputElement;
            const wanted = input.checked;
            input.checked = gateway.trusted_pairing ?? false;
            void setTrustedPairing(wanted);
          }} />
        <span class="toggle-slider" />
      </label>
    </div>
    <div class="settings-row">
      <div class="settings-row-label">
        Credential
        <div class="settings-row-note">{gateway.last_rotated_at ? `Rotated ${timeAgo(gateway.last_rotated_at, Date.now())}` : 'Rotates automatically every 30 days'}</div>
        {gateway.rotation_error && <div class="settings-row-note" role="status">{gateway.rotation_error} · Retries automatically.</div>}
        {!gateway.rotation_error && gateway.gateway_protocol < 2 && <div class="settings-row-note">Update the remote gateway to support credential rotation.</div>}
      </div>
      <button class="action-btn" disabled={saving} onClick={() => void rotate()}>Rotate</button>
    </div>
    <div class="settings-row">
      <div class="settings-row-label">
        Headers
        <div class="settings-row-note">{gateway.header_names.length ? gateway.header_names.map(n => `${n}: set`).join(' · ') : 'None'}</div>
      </div>
      <button class="action-btn" aria-expanded={editing} disabled={saving} onClick={() => setHeaders({ editing: !editing, rows: [] })}>Edit</button>
    </div>
    <Disclosure open={editing}>
      <p class="settings-row-note">Re-enter every header you want to keep. Saving replaces the entire set; an empty set clears it.</p>
      <RemoteHeadersEditor rows={rows} onChange={next => setHeaders({ editing, rows: next })} disabled={saving} />
      <div class="settings-row">
        <button class="action-btn" disabled={saving} onClick={() => void save()}>Save headers</button>
      </div>
    </Disclosure>
  </>;
}

function RemoteGatewayItem({
  gateway,
  state,
  reload,
  remove,
}: {
  gateway: RemoteGateway;
  state: RemoteGatewayState;
  reload: () => Promise<unknown>;
  remove: (gateway: RemoteGateway) => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [headers, setHeaders] = useState<HeadersDraft>(CLOSED_HEADERS_DRAFT);
  return (
    <div>
      <GatewayRow
        title={
          <button
            type="button"
            class="remote-gateway-toggle"
            aria-expanded={open}
            onClick={() => setOpen(!open)}
          >
            <DisclosureChevron open={open} />
            {gateway.name}
          </button>
        }
        detail={<RemoteGatewayDetail gateway={gateway} state={state} />}
      />
      <Disclosure open={state.kind === 'unreachable'}>
        {state.kind === 'unreachable' && <div class="settings-row-note" role="status">{state.reason}</div>}
      </Disclosure>
      <Disclosure open={state.kind === 'revoked'}>
        <RepairRemoteGateway gateway={gateway} reload={reload} />
      </Disclosure>
      <Disclosure open={open}>
        <div class="settings-row-child">
          <Disclosure open={state.kind !== 'revoked'}>
            <RemoteGatewaySettings gateway={gateway} reload={reload} headers={headers} setHeaders={setHeaders} />
          </Disclosure>
          <div class="settings-row">
            <button class="action-btn action-btn-danger" onClick={() => void remove(gateway)}>
              Remove {gateway.name}
            </button>
          </div>
        </div>
      </Disclosure>
    </div>
  );
}

export function RemoteGatewaysSection({ available }: { available: boolean }) {
  useEffect(() => { if (available) void loadPairedRemoteGateways(); }, [available]);
  const remotes = pairedRemoteGateways.value;
  const [listings, reloadListings] = useGatewayRead<RemoteGatewayListing>(available, listRemoteWorkspaces);
  const reload = useCallback(
    () => Promise.all([loadPairedRemoteGateways(), reloadListings()]),
    [reloadListings],
  );
  usePanelRefresh('remote gateways', reload);
  const [draft, setDraft] = useState<RemoteGatewayDraft>(EMPTY_REMOTE_GATEWAY_DRAFT);
  const [adding, setAdding] = useState(false);
  const [pairing, setPairing] = useState(false);
  const [awaitingOwner, setAwaitingOwner] = useState(false);
  const blocker = remoteGatewayBlocker(draft);
  const edit = (patch: Partial<RemoteGatewayDraft>) => setDraft((d) => ({ ...d, ...patch }));

  const pair = async () => {
    if (blocker !== null || pairing) return;
    setPairing(true);
    try {
      const added = await submitRemoteGateway(draft, () => setAwaitingOwner(true));
      setDraft(EMPTY_REMOTE_GATEWAY_DRAFT);
      setAdding(false);
      showToast(`Paired with ${added.name}`, 'success');
      await reload();
    } catch (e) {
      showToast(`Couldn't pair with that gateway: ${errorDetail(e)}`, 'error');
    } finally {
      setPairing(false);
      setAwaitingOwner(false);
    }
  };

  const remove = async (gateway: RemoteGateway) => {
    const ok = await showConfirm(
      `This gateway will stop opening ${gateway.name}'s workspaces. Pairing again takes a new gateway secret from ${gateway.name}.`,
      'Remove',
      { variant: 'danger' },
    );
    if (!ok) return;
    try {
      const removed = await removeRemoteGateway(gateway.name);
      if (!removed.remote_forgot && remoteGatewayState(listings, gateway).kind !== 'revoked') {
        showToast(
          `Removed ${gateway.name} here, but it did not answer: ${removed.detail ?? 'no reason given'}. Unpair this gateway there too.`,
          'warning',
        );
      }
      await reload();
    } catch (e) {
      showToast(`Couldn't remove ${gateway.name}: ${errorDetail(e)}`, 'error');
    }
  };

  const field = (
    label: string,
    key: 'address' | 'code' | 'name',
    placeholder: string,
    extra?: preact.ComponentChildren,
  ) => (
    <div class="settings-row update-address-row">
      <span class="settings-row-label">{label}</span>
      <div class="update-address-controls">
        <input
          class="settings-text-input update-address-input"
          value={draft[key]}
          placeholder={placeholder}
          aria-label={label}
          spellcheck={false}
          disabled={pairing}
          onInput={(e) => {
            const value = (e.currentTarget as HTMLInputElement).value;
            edit({ [key]: key === 'name' ? shortNameInput(value) : value });
          }}
          onKeyDown={(e) => { if (e.key === 'Enter') void pair(); }}
        />
        {extra}
      </div>
    </div>
  );

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="access:remote-gateways">
        Remote gateways
        <Explainer title="Remote gateways">
          <p>
            Opens another Lucidos gateway's workspaces from here, on every device paired
            with this gateway. On the other gateway, use Pair a gateway to get a secret.
          </p>
          <p>
            The other gateway needs an https address. The short name is how its
            workspaces are named here, as <code>name@short-name</code>.
          </p>
          <p>
            It is one-way: they cannot open yours, and their agents reach back here only
            where you turn that on. Each one can revoke your access by unpairing this
            gateway on its own Access page.
          </p>
        </Explainer>
      </div>
      {available ? (
        <>
          <div class="list-rows">
            <GatewayList
              list={remotes}
              noun="remote gateways"
              empty="No remote gateways yet."
              row={(gateway) => (
                <RemoteGatewayItem
                  key={gateway.name}
                  gateway={gateway}
                  state={remoteGatewayState(listings, gateway)}
                  reload={reload}
                  remove={remove}
                />
              )}
            />
          </div>
          {listings.status === 'failed' && (
            <LoadableError noun="whether remote gateways still answer" error={listings.error} />
          )}
          <button
            type="button"
            class="settings-disclosure-toggle"
            aria-expanded={adding}
            onClick={() => setAdding(!adding)}
          >
            <DisclosureChevron open={adding} />
            Add a remote gateway
          </button>
          <Disclosure open={adding}>
            <div class="settings-row-note" data-role="remote-gateway-trust">{TRUST_NOTE}</div>
            {field('Short name', 'name', 'cloud')}
            {field('Gateway pairing secret', 'code', 'lgp_…')}
            <RemoteHeadersEditor rows={draft.headers ?? []} onChange={headers => edit({ headers })} disabled={pairing} />
            {field(
              'Address',
              'address',
              'https://lucidos.example.com',
              <button
                class="action-btn action-btn-confirm"
                disabled={blocker !== null || pairing}
                onClick={() => void pair()}
              >
                {awaitingOwner ? 'Waiting…' : pairing ? 'Pairing…' : 'Pair'}
              </button>,
            )}
            {blocker?.kind === 'plain-http' && <div class="settings-row-note">{HTTPS_ONLY_NOTE}</div>}
            <Disclosure open={awaitingOwner}>
              <div class="settings-row-note" role="status">{AWAITING_OWNER_NOTE}</div>
            </Disclosure>
          </Disclosure>
        </>
      ) : (
        <div class="settings-section-desc">
          This page is served straight off the engine, so it cannot reach the gateway that
          pairs with other gateways. Open Lucidos through its gateway to add one.
        </div>
      )}
    </div>
  );
}
