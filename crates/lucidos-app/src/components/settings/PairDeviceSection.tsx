

import { useCallback, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { mintPairingCode } from '../../api/client/pairing';
import { LoadableError } from '../shared/LoadableError';
import { Explainer } from '../shared/Explainer';
import { copyToClipboard } from '../../utils/clipboard';
import { clipboardAbilities } from '../../utils/platform';
import { toFailed } from '../../store/types';
import type { Loadable } from '../../store/types';
import { useCodeCountdown } from './useCodeCountdown';
import { Dropdown } from '../shared/Dropdown';
import { isTeamGateway, gatewayUserRows, gatewayUsers } from '../../store/actions/gatewayUsers';

export interface MintedCode {
  code: string;
  pairUrl: string | null;
  qrSvg: string | null;
  expiresAt: number;
}

export type PairTarget =
  | { kind: 'resolving' }
  | { kind: 'unknown' }
  | { kind: 'none' }
  | { kind: 'origin'; url: string };

export function qrImageSrc(svg: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

export function targetSentence(target: PairTarget): ComponentChildren {
  switch (target.kind) {
    case 'resolving':
      return 'Working out which address a new device should open.';
    case 'unknown':
      return 'Could not work out an address for a QR, so this will mint the code alone. The error under Other addresses says why.';
    case 'none':
      return 'Creates a one-time code. No address here reaches another device, so there is nothing to put in a QR: open Lucidos on the other device and type the code in.';
    case 'origin':
      return (
        <>
          Creates a one-time code, and a QR pointing at <strong>{target.url}</strong> for the
          new device to scan.
        </>
      );
  }
}

export function expiryLabel(left: number): string {
  if (left <= 0) return 'This code has expired';
  if (left < 60) return `Expires in ${left}s`;
  const minutes = Math.floor(left / 60);
  const seconds = left % 60;
  return `Expires in ${minutes}:${String(seconds).padStart(2, '0')}`;
}

export function PairDeviceSection({
  canMint,
  target,
}: {
  canMint: boolean;
  target: PairTarget;
}) {
  const [minted, setMinted] = useState<Loadable<MintedCode>>({ status: 'not-loaded' });
  const origin = target.kind === 'origin' ? target.url : null;
  const users = isTeamGateway.value ? gatewayUserRows(gatewayUsers.value) : null;
  const [chosenUser, setChosenUser] = useState<string | null>(null);
  const forUser = users === null ? null : chosenUser ?? users[0]?.id ?? null;

  const mint = useCallback(() => {
    setMinted({ status: 'loading' });
    void mintPairingCode(origin, forUser).then(
      (body) => {
        const landed = Date.now();
        setMinted({
          status: 'loaded',
          data: {
            code: body.code,
            pairUrl: body.pair_url ?? null,
            qrSvg: body.qr_svg ?? null,
            expiresAt: landed + body.expires_in_secs * 1000,
          },
        });
      },
      (e) => setMinted(toFailed(e)),
    );
  }, [origin, forUser]);

  const left = useCodeCountdown(minted.status === 'loaded' ? minted.data.expiresAt : null);

  function body() {
    if (minted.status === 'failed') {
      return (
        <div class="list-row">
          <div class="list-row-info">
            <LoadableError noun="a pairing code" error={minted.error} />
          </div>
          <div class="list-row-actions">
            <button class="action-btn" onClick={mint}>Try again</button>
          </div>
        </div>
      );
    }
    if (minted.status !== 'loaded') {
      const resolving = target.kind === 'resolving';
      return (
        <div class="list-row">
          <div class="list-row-info">
            <div class="title">Pair a phone or another computer</div>
            <div class="list-row-details list-row-details-prose">
              {targetSentence(target)}
            </div>
          </div>
          <div class="list-row-actions">
            {users !== null && forUser !== null && (
              <Dropdown
                options={users.map((o) => ({ value: o.id, label: `For ${o.name}` }))}
                value={forUser}
                onChange={setChosenUser}
              />
            )}
            <button
              class="action-btn action-btn-confirm"
              disabled={resolving || minted.status === 'loading'}
              onClick={mint}
            >
              {minted.status === 'loading' ? 'Creating…' : 'Create a pairing code'}
            </button>
          </div>
        </div>
      );
    }
    return <MintedBody minted={minted.data} left={left} onAgain={mint} />;
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="access:pair-device">
        Pair a device
        <Explainer title="Pair a device">
          <p>
            Lucidos answers only devices you have paired. This mints a one-time code
            and draws it as a QR, so a phone scans instead of typing eight digits.
          </p>
          <p>
            You can do this from here because this device is already paired. The other
            way is <code>lucidos pair</code> in a terminal on the machine.
          </p>
        </Explainer>
      </div>
      {canMint ? (
        <div class="list-rows">{body()}</div>
      ) : (
        <div class="settings-section-desc">
          This page is served straight off the engine, so it cannot reach the pairing
          service. Run <code>lucidos pair</code> in a terminal on the machine running
          Lucidos, then enter the code on the new device.
        </div>
      )}
    </div>
  );
}

export function PairCard({
  kind,
  label,
  action,
  children,
}: {
  kind: 'qr' | 'code' | 'url';
  label: string;
  action?: ComponentChildren;
  children: ComponentChildren;
}) {
  return (
    <div class={`add-device-card add-device-card-${kind}`}>
      <div class="add-device-card-head">
        <span class="add-device-card-label">{label}</span>
        {action}
      </div>
      {children}
    </div>
  );
}

export function CopyButton({ text, what }: { text: string; what: string }) {
  if (!clipboardAbilities().copy) return null;
  return (
    <button class="action-btn" aria-label={`Copy the ${what}`} onClick={() => copyToClipboard(text)}>
      Copy
    </button>
  );
}

function MintedBody({
  minted,
  left,
  onAgain,
}: {
  minted: MintedCode;
  left: number;
  onAgain: () => void;
}) {
  const expired = left <= 0;
  const scanCard = minted.qrSvg ? (
    <PairCard kind="qr" label="Scan this">
      <img class="add-device-qr" src={qrImageSrc(minted.qrSvg)} alt="" />
    </PairCard>
  ) : null;
  return (
    <div class="add-device">
      <div class="add-device-status">
        <span class="add-device-expiry">{expiryLabel(left)}</span>
        <button class="action-btn" onClick={onAgain}>
          {expired ? 'Create another' : 'New code'}
        </button>
      </div>
      {!expired && (
        <>
          <div class="add-device-cards">
            {scanCard}
            <div class="add-device-alts">
              <PairCard
                kind="code"
                label={scanCard ? 'Or type this code' : 'Type this code'}
                action={<CopyButton text={minted.code} what="pairing code" />}
              >
                <div class="add-device-code">{minted.code}</div>
              </PairCard>
              {minted.pairUrl && (
                <PairCard
                  kind="url"
                  label="Or open this address"
                  action={<CopyButton text={minted.pairUrl} what="pairing address" />}
                >
                  <div class="add-device-url">{minted.pairUrl}</div>
                </PairCard>
              )}
            </div>
          </div>
          {!minted.pairUrl && (
            <div class="add-device-note">
              Open Lucidos on the other device and enter the code there.
            </div>
          )}
        </>
      )}
    </div>
  );
}
