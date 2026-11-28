
import { AWAITING_OWNER, isAwaitingOwner, untilOwnerAllows, type AwaitingOwner } from './awaitingOwner';
import { gatewayErrorReason } from './gatewayError';
import { isTauri } from '../../utils/platform';
import { invoke } from '../../utils/tauri';

const AUTH = '/~/api/v1/auth';

const DESKTOP_DEVICE_LABEL = 'Lucidos desktop';

export interface PairingCode {
  code: string;
  expires_in_secs: number;
  pair_url?: string;
  qr_svg?: string;
}

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

export interface PairingSession {
  paired: boolean;
  device_id?: string;
  device_label?: string;
  local: boolean;
}

export async function pairingSession(): Promise<PairingSession> {
  const res = await fetch(`${AUTH}/session`, { credentials: 'same-origin' });
  if (!res.ok) throw new Error(await gatewayErrorReason(res));
  return res.json() as Promise<PairingSession>;
}

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

export async function redeemPairingCode(
  code: string,
  label?: string,
  onWaiting: () => void = () => {},
  signal?: AbortSignal,
): Promise<void> {
  await untilOwnerAllows(() => redeemOnce(code, label), onWaiting, signal);
}

export async function pairDesktopWindow(onWaiting: () => void = () => {}): Promise<void> {
  if (!isTauri()) throw new Error('not running in the Lucidos desktop app');
  const minted = await invoke<{ code: string }>('mint_pairing_code');
  const code = minted?.code?.trim();
  if (!code) throw new Error('the desktop app minted no pairing code');
  await redeemPairingCode(code, DESKTOP_DEVICE_LABEL, onWaiting);
}
