/**
 * "Add a device" on Settings -> Access: a pairing QR a phone can scan.
 *
 * The gateway answers no device it has not been paired with (ADR 0094), and
 * pairing means an eight-digit code. Typing one off a laptop screen into a
 * phone is the tedious part. So this mints a code and draws a QR for
 * `<reachable-origin>/~/?pair=<code>`. The phone's camera opens it, the picker
 * loads, and the code is already in the box.
 *
 * **Anyone reading this page is already paired**, which is what makes minting
 * here safe to offer. A paired device holds full authority, so it may enrol
 * another one.
 *
 * A code lasts five minutes and is spent once. When it runs out the section
 * says so, drops the cards, and waits for the button.
 */

// Four things this will not do, each for a reason:
//
// - **It never mints a code the reader did not ask for.** It used to replace an
//   expired code by itself, which read as the page undoing the press the reader
//   had just made. ADR 0098.
// - **It never guesses an address.** `pairingOrigin` answers `null` when
//   nothing reachable was derived, and the section then shows the code alone.
//   A QR pointing at `127.0.0.1` is worse than no QR.
// - **It does not name the device.** The phone names itself on the pairing
//   screen, and that name wins over one minted here.
// - **It does not hide when the gateway is out of reach.** The shell and its
//   search anchor render either way, for the reason Connect URLs records: an
//   absent section drops a Search Everywhere hit at the top of the page.

import { useCallback, useEffect, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { mintPairingCode } from '../../api/client/pairing';
import { LoadableError } from '../shared/LoadableError';
import { Explainer } from '../shared/Explainer';
import { copyToClipboard } from '../../utils/clipboard';
import { clipboardAbilities } from '../../utils/platform';
import { toFailed } from '../../store/types';
import type { Loadable } from '../../store/types';

/** A minted code, and the moment it stops working. */
export interface MintedCode {
  code: string;
  pairUrl: string | null;
  qrSvg: string | null;
  expiresAt: number;
}

/** What the page knows about where to send a new device.
 *
 *  Four states, and `unknown` is not a synonym for `none`. The page must not
 *  claim no address reaches another device when the read that would say so
 *  failed. Same distinction `HostTailnetState` draws, for the same reason.
 *
 *  `resolving` is the one that bites without this type. The reads take a
 *  moment, and clicking inside that window would mint a bare code on a machine
 *  with a perfectly good address. */
export type PairTarget =
  | { kind: 'resolving' }
  | { kind: 'unknown' }
  | { kind: 'none' }
  | { kind: 'origin'; url: string };

/** Pure: the SVG the gateway rendered, as something an `img` can load.
 *
 *  A `data:` URL rather than inline markup, so the QR goes through the image
 *  path and can run nothing. An SVG loaded by `img` gets no script and no
 *  external fetch, whatever it holds. This one holds only rectangles, which
 *  the gateway's own test pins. Both together are what make a server-rendered
 *  image safe to drop into the page. */
export function qrImageSrc(svg: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

/** Pure: what the idle row says about where a new device will be sent.
 *
 *  Exported so all four states are testable, and so the `unknown` wording
 *  cannot quietly drift back into claiming there is no address. */
export function targetSentence(target: PairTarget): ComponentChildren {
  switch (target.kind) {
    case 'resolving':
      return 'Working out which address a new device should open.';
    case 'unknown':
      return 'Could not work out an address for a QR, so this will mint the code alone. The error above says why.';
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

/** Pure: how long is left, in whole seconds, never below zero. */
export function secondsLeft(expiresAt: number, now: number): number {
  return Math.max(0, Math.ceil((expiresAt - now) / 1000));
}

/** Pure: the expiry line, which has to keep saying something true.
 *
 *  A code sitting on a screen for ten minutes stops working silently, and a
 *  static "expires in five minutes" is a lie by then. The countdown is the
 *  honest version, and zero is its own state rather than a very small number.
 *
 *  Single use is deliberately NOT said here. It is a property of the code that
 *  nobody has to act on: the reader mints one, hands it over, and it is spent.
 *  Saying it beside a ticking clock made the one line that has to be read every
 *  second twice as long as the fact it carries. */
export function expiryLabel(left: number): string {
  if (left <= 0) return 'This code has expired';
  if (left < 60) return `Expires in ${left}s`;
  const minutes = Math.floor(left / 60);
  const seconds = left % 60;
  return `Expires in ${minutes}:${String(seconds).padStart(2, '0')}`;
}

/**
 * The section itself.
 *
 * `canMint` is whether `/~/…` resolves to the gateway from this page, which is
 * true exactly while we are served under `/<slug>/`. `target` is where a new
 * device should point, in one of the four states above.
 */
export function AddDeviceSection({
  canMint,
  target,
}: {
  canMint: boolean;
  target: PairTarget;
}) {
  const [minted, setMinted] = useState<Loadable<MintedCode>>({ status: 'not-loaded' });
  const [now, setNow] = useState(() => Date.now());
  const origin = target.kind === 'origin' ? target.url : null;

  // Mints a code, and every button in the section is wired to it.
  const mint = useCallback(() => {
    setMinted({ status: 'loading' });
    void mintPairingCode(origin).then(
      (body) => {
        // The clock is read HERE, beside the expiry it is compared against.
        // Stamping it when the request went out would open the countdown at
        // however long the round trip took, so "5:02" on a slow one.
        const landed = Date.now();
        setNow(landed);
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
  }, [origin]);

  // Ticks only while a live code is on screen, and stops itself once that code
  // is dead. A countdown is the one thing here that has to keep moving.
  const live = minted.status === 'loaded';
  const left = live ? secondsLeft(minted.data.expiresAt, now) : 0;
  const dead = left <= 0;
  useEffect(() => {
    if (!live || dead) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [live, dead]);

  // A hidden tab throttles the interval above, so `now` comes back stale and
  // the countdown would show time a dead code no longer has. Re-read the clock
  // on the way in.
  useEffect(() => {
    const onVisibility = () => {
      if (document.visibilityState !== 'hidden') setNow(Date.now());
    };
    document.addEventListener('visibilitychange', onVisibility);
    return () => document.removeEventListener('visibilitychange', onVisibility);
  }, []);

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
      // Minting is held until the address is known. Clicking before the reads
      // land would burn a code and show no QR, on a machine that had a
      // perfectly good address for one.
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
      <div class="settings-section-title" data-search-anchor="access:add-device">
        Add a device
        <Explainer title="Add a device">
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

/** One way to use the code: a caption, an optional action, and the thing itself.
 *
 *  `kind` names the card in the DOM, and is closed rather than a bare string so
 *  a typo cannot invent a fourth. The QR and the code each take a rule of their
 *  own; the address card rides the base class alone today. */
function PairCard({
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

/** Copy one of the two things a card can hand over.
 *
 *  Renders nothing where there is no clipboard to copy to. A non-secure context
 *  has none at all. This page is often read over a plain-HTTP LAN address,
 *  where the button would fail on every tap. The digits and the address stay
 *  selectable either way. */
function CopyButton({ text, what }: { text: string; what: string }) {
  if (!clipboardAbilities().copy) return null;
  return (
    <button class="action-btn" aria-label={`Copy the ${what}`} onClick={() => copyToClipboard(text)}>
      Copy
    </button>
  );
}

/**
 * A live code, as the three ways to use it.
 *
 * **The QR is the point**, so it is the big card and the two fallbacks stack
 * beside it. They are ALTERNATIVES, not steps, which the captions have to carry:
 * every card after the first opens with "Or", or a row of imperatives reads as a
 * checklist of three things to do.
 *
 * Above them sits what is true of the code itself rather than of one way to
 * spend it: how long it has left, and the button that replaces it.
 *
 * **An expired code shows no cards at all.** Every one of them is an
 * instruction, and there is nothing left for the digits or the QR to do. The
 * line above says so, beside the button that fixes it.
 */
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
  // The QR card, or nothing. Built here so the caption below can ask the CARD
  // whether it exists, rather than re-deriving it: two conditions could
  // disagree, and a lone card would then open with "Or".
  // The address never has to ask. The gateway derives a QR from the pair URL,
  // so an address on screen means a QR above it.
  const scanCard = minted.qrSvg ? (
    <PairCard kind="qr" label="Scan this">
      {/* Decorative: the digits and the address beside it say the same thing in
          text, and nobody scans a QR with a screen reader. */}
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
                  {/* Text, never a link. The address is for the OTHER device,
                      and opening it here would spend a single-use code on a
                      device that is already paired. */}
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
