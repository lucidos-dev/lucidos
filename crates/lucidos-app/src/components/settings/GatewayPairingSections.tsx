/**
 * Gateway pairing on Settings → Access (ADR 0590): two sections.
 *
 * - **Pair a gateway** is this gateway as B. It mints a gateway pairing code,
 *   and lists the gateways that may open this one's workspaces, with Unpair.
 * - **Remote gateways** is this gateway as A. It lists the gateways it has
 *   paired with, with Remove, and takes the address, code and short name of a
 *   new one.
 *
 * Both live on the gateway's own surface, so off the gateway they render their
 * shell and say what to run instead, like "Add a device".
 */

import { useCallback, useEffect, useState } from 'preact/hooks';
import {
  listPairedGateways,
  listRemoteGateways,
  mintGatewayPairingCode,
  removeRemoteGateway,
  unpairGateway,
  type PairedGateway,
  type RemoteGateway,
} from '../../api/client/control';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { showConfirm, showToast } from '../../store/store';
import { loadingIfFresh, toFailed, type Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { Explainer } from '../shared/Explainer';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { CopyButton, PairCard, expiryLabel, type PairTarget } from './AddDeviceSection';
import { timeAgo } from './pairedDevices';
import {
  EMPTY_REMOTE_GATEWAY_DRAFT,
  HTTPS_ONLY_NOTE,
  TRUST_NOTE,
  remoteGatewayBlocker,
  shortNameInput,
  submitRemoteGateway,
  type RemoteGatewayDraft,
} from './remoteGatewayForm';
import { useCodeCountdown } from './useCodeCountdown';

/** Pure: the https address another gateway should pair with, or `null`.
 *
 *  Only an `https` pairing origin qualifies, because gateways pair over https
 *  only. A plain-http one would be refused on the other side. */
export function gatewayPairingAddress(target: PairTarget): string | null {
  return target.kind === 'origin' && target.url.startsWith('https://') ? target.url : null;
}

/** A list read this section owns, with its refresh. */
function useGatewayList<T>(
  available: boolean,
  read: () => Promise<T[]>,
  name: string,
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
  usePanelRefresh(name, reload);
  return [list, reload];
}

/** One row of either list: a name, a line under it, one action. With no
 *  props, inside a skeleton, it is the loading placeholder. */
function GatewayRow({
  title,
  detail,
  action,
}: {
  title?: string;
  detail?: string;
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

/** A list under a delay-gated skeleton, its failure, or its rows. */
function GatewayList<T>({
  list,
  noun,
  empty,
  row,
}: {
  list: Loadable<T[]>;
  noun: string;
  empty: string;
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
          ? <div class="settings-section-desc">{empty}</div>
          : list.data.map(row)
      )}
    </LoadingFade>
  );
}

/** A minted gateway code and when it stops working. */
interface MintedGatewayCode {
  code: string;
  expiresAt: number;
}

/** "Pair a gateway": this gateway lets another one in. */
export function PairGatewaySection({
  available,
  target,
}: {
  available: boolean;
  target: PairTarget;
}) {
  const [minted, setMinted] = useState<Loadable<MintedGatewayCode>>({ status: 'not-loaded' });
  const [paired, reload] = useGatewayList<PairedGateway>(
    available,
    listPairedGateways,
    'pair a gateway',
  );
  const left = useCodeCountdown(minted.status === 'loaded' ? minted.data.expiresAt : null);
  const address = gatewayPairingAddress(target);

  const mint = () => {
    setMinted({ status: 'loading' });
    mintGatewayPairingCode().then(
      (body) => setMinted({
        status: 'loaded',
        data: { code: body.code, expiresAt: Date.now() + body.expires_in_secs * 1000 },
      }),
      (e) => setMinted(toFailed(e)),
    );
  };

  const unpair = async (gateway: PairedGateway) => {
    const ok = await showConfirm(
      `${gateway.label} will no longer open this gateway's workspaces. Pairing again takes a new gateway code.`,
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

  function codeBody() {
    if (minted.status === 'failed') {
      return (
        <div class="list-row">
          <div class="list-row-info">
            <LoadableError noun="a gateway pairing code" error={minted.error} />
          </div>
          <div class="list-row-actions">
            <button class="action-btn" onClick={mint}>Try again</button>
          </div>
        </div>
      );
    }
    if (minted.status !== 'loaded') {
      return (
        <div class="list-row">
          <div class="list-row-info">
            <div class="title">Let another gateway in</div>
            <div class="list-row-details list-row-details-prose">
              Creates a one-time code. The other gateway enters it under Remote gateways.
            </div>
          </div>
          <div class="list-row-actions">
            <button
              class="action-btn action-btn-confirm"
              disabled={minted.status === 'loading'}
              onClick={mint}
            >
              {minted.status === 'loading' ? 'Creating…' : 'Create a gateway code'}
            </button>
          </div>
        </div>
      );
    }
    const expired = left <= 0;
    return (
      <div class="add-device">
        <div class="add-device-status">
          <span class="add-device-expiry">{expiryLabel(left)}</span>
          <button class="action-btn" onClick={mint}>{expired ? 'Create another' : 'New code'}</button>
        </div>
        {!expired && (
          <div class="add-device-cards">
            <div class="add-device-alts">
              <PairCard
                kind="code"
                label="Enter this code on the other gateway"
                action={<CopyButton text={minted.data.code} what="gateway pairing code" />}
              >
                <div class="add-device-code">{minted.data.code}</div>
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
            paired with it. The other gateway enters the code under Remote gateways.
          </p>
          <p>
            It is one-way: this gateway gains nothing on the other one. Unpair ends it on
            the other gateway's next request.
          </p>
        </Explainer>
      </div>
      {available ? (
        <div class="list-rows">
          {codeBody()}
          <GatewayList
            list={paired}
            noun="paired gateways"
            empty="No other gateway can open this gateway's workspaces."
            row={(gateway) => (
              <GatewayRow
                key={gateway.id}
                title={gateway.label}
                detail={`Paired ${timeAgo(gateway.paired_at, Date.now())} · can open this gateway's workspaces`}
                action={
                  <button class="action-btn action-btn-danger" onClick={() => void unpair(gateway)}>
                    Unpair
                  </button>
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

/** "Remote gateways": this gateway opens another one's workspaces. */
export function RemoteGatewaysSection({ available }: { available: boolean }) {
  const [remotes, reload] = useGatewayList<RemoteGateway>(
    available,
    listRemoteGateways,
    'remote gateways',
  );
  const [draft, setDraft] = useState<RemoteGatewayDraft>(EMPTY_REMOTE_GATEWAY_DRAFT);
  const [pairing, setPairing] = useState(false);
  const blocker = remoteGatewayBlocker(draft);
  const edit = (patch: Partial<RemoteGatewayDraft>) => setDraft((d) => ({ ...d, ...patch }));

  const pair = async () => {
    if (blocker !== null || pairing) return;
    setPairing(true);
    try {
      const added = await submitRemoteGateway(draft);
      setDraft(EMPTY_REMOTE_GATEWAY_DRAFT);
      showToast(`Paired with ${added.name}`, 'success');
      await reload();
    } catch (e) {
      showToast(`Couldn't pair with that gateway: ${errorDetail(e)}`, 'error');
    } finally {
      setPairing(false);
    }
  };

  const remove = async (gateway: RemoteGateway) => {
    const ok = await showConfirm(
      `This gateway will stop opening ${gateway.name}'s workspaces. Pairing again takes a new gateway code from ${gateway.name}.`,
      'Remove',
      { variant: 'danger' },
    );
    if (!ok) return;
    try {
      const removed = await removeRemoteGateway(gateway.name);
      if (!removed.remote_forgot) {
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
    key: keyof RemoteGatewayDraft,
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
            with this gateway. On the other gateway, use Pair a gateway to get a code.
          </p>
          <p>
            The other gateway needs an https address. The short name is how its
            workspaces are named here, as <code>name@short-name</code>.
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
                <GatewayRow
                  key={gateway.name}
                  title={gateway.name}
                  detail={`${gateway.address} · added ${timeAgo(gateway.added_at, Date.now())}`}
                  action={
                    <button class="action-btn action-btn-danger" onClick={() => void remove(gateway)}>
                      Remove
                    </button>
                  }
                />
              )}
            />
          </div>
          <div class="settings-row-note" data-role="remote-gateway-trust">{TRUST_NOTE}</div>
          {field('Address', 'address', 'https://lucidos.example.com')}
          {field('Gateway pairing code', 'code', '8 digits')}
          {field(
            'Short name',
            'name',
            'cloud',
            <button
              class="action-btn action-btn-confirm"
              disabled={blocker !== null || pairing}
              onClick={() => void pair()}
            >
              {pairing ? 'Pairing…' : 'Pair'}
            </button>,
          )}
          {blocker?.kind === 'plain-http' && <div class="settings-row-note">{HTTPS_ONLY_NOTE}</div>}
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
