/**
 * Pairing codes, from the gateway's own surface (ADR 0094).
 *
 * `/~/api/v1/auth/*` is an absolute path, exactly as the control plane's is,
 * and for the same reason: `~` is the reserved sigil that can never be a
 * workspace slug, so one call works from the picker and from inside a
 * workspace. A page served off a direct engine port resolves it against the
 * engine and gets a 404, which is what callers gate on.
 *
 * Minting is gated server-side, and an already-paired device may do it. So
 * Settings -> Access can add a phone with nobody walking back to a terminal.
 */

import { AWAITING_OWNER, isAwaitingOwner, untilOwnerAllows, type AwaitingOwner } from './awaitingOwner';
import { gatewayErrorReason } from './gatewayError';
import { isTauri } from '../../utils/platform';
import { invoke } from '../../utils/tauri';

const AUTH = '/~/api/v1/auth';

/** What the desktop window calls itself in the paired-device list.
 *
 * Fixed rather than asked for. The window pairs itself on first launch, and
 * stopping to name the machine you are sitting at buys nothing. */
const DESKTOP_DEVICE_LABEL = 'Lucidos desktop';

/** A freshly minted code, and the QR for it when an origin was sent.
 *
 * `pair_url` and `qr_svg` arrive together or not at all. They are absent when
 * no reachable origin could be derived, which is a normal answer rather than a
 * failure: the code alone still pairs a device that knows an address. */
export interface PairingCode {
  code: string;
  expires_in_secs: number;
  pair_url?: string;
  qr_svg?: string;
}

/**
 * Mint a one-time pairing code, and a QR for `origin` when there is one.
 *
 * `origin` is the address the NEW device should open, never this page's own.
 * The caller resolves it, because only the client knows which of this
 * machine's addresses another device can reach. Pass `null` to mint a bare
 * code. `userId` names the user the new device joins; without it the
 * device joins the gateway's first person.
 */
export async function mintPairingCode(
  origin: string | null,
  userId: string | null = null,
): Promise<PairingCode> {
  const params = new URLSearchParams();
  if (origin) params.set('origin', origin);
  if (userId) {
    params.set('user', userId);
    // See "User rename wire aliases" in docs/temporary-measures.md.
    params.set('operator', userId);
  }
  const query = params.size > 0 ? `?${params}` : '';
  const res = await fetch(`${AUTH}/pairing-code${query}`, {
    method: 'POST',
    credentials: 'same-origin',
  });
  if (!res.ok) throw new Error(await gatewayErrorReason(res));
  return res.json() as Promise<PairingCode>;
}

/** What the gateway makes of this caller.
 *
 * Public, so a client with no credential can ask before it is refused. The
 * device id is what a caller compares against the paired list to recognise
 * itself. */
export interface PairingSession {
  paired: boolean;
  device_id?: string;
  device_label?: string;
  local: boolean;
}

/** Ask the gateway who is calling. */
export async function pairingSession(): Promise<PairingSession> {
  const res = await fetch(`${AUTH}/session`, { credentials: 'same-origin' });
  if (!res.ok) throw new Error(await gatewayErrorReason(res));
  return res.json() as Promise<PairingSession>;
}

/**
 * Spend a code once, so this browser becomes a paired device.
 *
 * The credential arrives as an `HttpOnly` cookie, so there is nothing to store.
 * The browser sends it from here on. [`AWAITING_OWNER`] means the code waits
 * for a paired device's Allow, and the caller presents it again.
 */
async function redeemOnce(code: string, label?: string): Promise<'paired' | AwaitingOwner> {
  const res = await fetch(`${AUTH}/pair`, {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ code, label }),
  });
  if (isAwaitingOwner(res)) return AWAITING_OWNER;
  if (!res.ok) throw new Error(await gatewayErrorReason(res));
  return 'paired';
}

/**
 * Spend a code, waiting for a paired device's Allow when the code needs one.
 *
 * `onWaiting` runs each time the gateway says it waits, so the screen can say
 * so. Resolves once paired, and rejects on a refusal: a wrong or expired code,
 * or Don't allow.
 */
export async function redeemPairingCode(
  code: string,
  label?: string,
  onWaiting: () => void = () => {},
  signal?: AbortSignal,
): Promise<void> {
  await untilOwnerAllows(() => redeemOnce(code, label), onWaiting, signal);
}

/**
 * Pair the desktop window, with no code typed anywhere.
 *
 * The window is `WebviewUrl::External` against the gateway, so it is a browser
 * and starts unpaired like a phone. Its Rust side can read the machine-local
 * token, though, which makes it a pairing authority (ADR 0094). That side
 * mints; this redeems, so the cookie lands in this webview's own jar.
 *
 * Rejects off Tauri, and whenever the mint or the redemption failed. The caller
 * falls back to the typed form rather than treating it as a dead end.
 *
 * The mint goes through the local token, so once another device is paired the
 * window waits for that device's Allow. `onWaiting` lets the screen say so.
 */
export async function pairDesktopWindow(onWaiting: () => void = () => {}): Promise<void> {
  if (!isTauri()) throw new Error('not running in the Lucidos desktop app');
  const minted = await invoke<{ code: string }>('mint_pairing_code');
  const code = minted?.code?.trim();
  if (!code) throw new Error('the desktop app minted no pairing code');
  await redeemPairingCode(code, DESKTOP_DEVICE_LABEL, onWaiting);
}
