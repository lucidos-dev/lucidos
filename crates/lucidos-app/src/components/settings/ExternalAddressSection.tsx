import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';
import { showToast } from '../../store/store';
import { setExternalAddress, type ExternalAddressInfo } from '../../api/client/control';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useServerBackedField } from '../../hooks/useServerBackedField';
import type { Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { Explainer } from '../shared/Explainer';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { SkBlock, SkText, SkeletonProvider } from '../shared/Skeleton';

/** What the placeholder shows when nothing tells us the default. */
const EXAMPLE_ORIGIN = 'https://lucidos.example.com';

/** Pure: the sentence under the field, from the gateway's answer.
 *
 *  A probe verdict leads, since it answers the save the user just made. Then
 *  what the stored value means for the pairing QR. `tailnetDefault` is the
 *  origin pairing falls back to, so the empty state can say it is Tailscale. */
export function externalAddressNote(info: ExternalAddressInfo, tailnetDefault: string | null): string {
  if (info.locked) {
    return `Set by ${info.env_var} in the gateway's environment, so it cannot be changed here.`;
  }
  const probe = info.probe;
  if (probe?.verdict === 'verified') return 'Checked: it reaches this gateway.';
  if (probe?.verdict === 'unreachable') {
    return `Saved, but it did not answer from this machine (${probe.detail}). That is normal behind hairpin NAT or a client-only VPN, so test it from a phone.`;
  }
  if (probe?.verdict === 'unknown') return `Saved, but it could not be checked (${probe.detail}).`;
  if (info.external_address !== null) return 'Pairing QRs use this address ahead of Tailscale.';
  return tailnetDefault !== null
    ? 'Empty, so pairing uses your Tailscale address.'
    : 'Empty, so pairing uses Tailscale once this machine is on a tailnet.';
}

/** The External address section of Settings → Access (ADR 0550).
 *
 *  The setting is the GATEWAY's, one per gateway, so this reads and writes it
 *  through the gateway's control plane. `available` is false on a page served
 *  straight off an engine port, which cannot reach that plane. */
export function ExternalAddressSection({
  available,
  info,
  tailnetDefault,
  onSaved,
}: {
  available: boolean;
  info: Loadable<ExternalAddressInfo>;
  tailnetDefault: string | null;
  onSaved: (info: ExternalAddressInfo) => void;
}) {
  const showLoading = useDelayedLoading(info);
  // The shell and its anchor render in every state, for the reason
  // `NetworkAccessPage` gives for `access:network`.
  const shell = (body: ComponentChildren) => (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="access:external-address">
        External address
        <Explainer title="External address">
          <p>
            The address other devices open to reach this machine, through a route you run:
            a reverse proxy, WireGuard, a company VPN, or a company name with its own
            certificate.
          </p>
          <p>
            When it is set, pairing QRs use it ahead of Tailscale. Empty, they use your
            Tailscale address.
          </p>
          <p>
            Saving checks that the address reaches this gateway. Only an answer from a
            different Lucidos gateway refuses it, since a code minted here would not work
            there.
          </p>
          <p>
            Pairing is the only gate Lucidos puts in front of it. If the address is public,
            put an identity proxy or mTLS in front of it as well.
          </p>
        </Explainer>
      </div>
      {body}
    </div>
  );
  if (!available) {
    return shell(
      <p class="settings-section-desc">
        This page is served straight off the engine, so it cannot reach the gateway's
        settings. Run <code>lucidos external-address</code> in a terminal on the machine
        running Lucidos.
      </p>,
    );
  }
  if (info.status === 'failed') {
    return shell(<LoadableError noun="external address" error={info.error} />);
  }
  return shell(
    <LoadingFade
      showSkeleton={showLoading}
      skeleton={<SkeletonProvider><ExternalAddressRow /></SkeletonProvider>}
    >
      {info.status === 'loaded' && (
        <ExternalAddressRow
          key={info.data.external_address ?? ''}
          info={info.data}
          tailnetDefault={tailnetDefault}
          onSaved={onSaved}
        />
      )}
    </LoadingFade>,
  );
}

/** The field, its Save button and the note. Keyed on the stored value by its
 *  caller, so a save or a CLI change redraws it from the gateway. With no
 *  `info`, inside a `SkeletonProvider`, it is the loading placeholder. */
function ExternalAddressRow({
  info,
  tailnetDefault = null,
  onSaved,
}: {
  info?: ExternalAddressInfo;
  tailnetDefault?: string | null;
  onSaved?: (info: ExternalAddressInfo) => void;
}) {
  const stored = info?.external_address ?? '';
  const [draft, setDraft] = useServerBackedField(stored);
  const [saving, setSaving] = useState(false);
  const locked = info?.locked ?? false;
  const changed = draft.trim() !== stored;

  const save = async () => {
    if (!changed || saving || !onSaved) return;
    setSaving(true);
    try {
      onSaved(await setExternalAddress(draft.trim()));
    } catch (e) {
      showToast(`Couldn't save the external address: ${errorDetail(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  };

  return (
    <div class="settings-row update-address-row">
      <SkText class="settings-row-label" w="12rem">Address other devices open</SkText>
      <div class="update-address-controls">
        <SkBlock w="100%" h="2rem">
          <input
            class="settings-text-input update-address-input"
            type="url"
            value={draft}
            disabled={locked}
            placeholder={tailnetDefault ?? EXAMPLE_ORIGIN}
            aria-label="External address"
            spellcheck={false}
            onInput={(e) => setDraft((e.currentTarget as HTMLInputElement).value)}
            onKeyDown={(e) => { if (e.key === 'Enter') void save(); }}
          />
        </SkBlock>
        <SkBlock w="3.5rem" h="2rem" round>
          <button class="action-btn" disabled={locked || !changed || saving} onClick={() => { void save(); }}>
            {saving ? 'Checking…' : 'Save'}
          </button>
        </SkBlock>
      </div>
      <SkText class="settings-row-note" as="div" w="80%">
        {info && externalAddressNote(info, tailnetDefault)}
        {info && !locked && info.suggestions.length > 0 && (
          <>
            {' '}Other addresses here:{' '}
            {info.suggestions.map((s, i) => (
              <span key={s.origin}>
                {i > 0 && ' · '}
                <button class="accent-link" onClick={() => setDraft(s.origin)}>
                  {s.origin.replace(/^https?:\/\//, '').replace(/:\d+$/, '')}
                </button>{' '}
                ({s.interface})
              </span>
            ))}
          </>
        )}
      </SkText>
    </div>
  );
}
