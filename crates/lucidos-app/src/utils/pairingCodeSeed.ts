
import {
  DEVICE_PAIRING_SECRET_PREFIX,
  PAIR_PARAM,
  PAIRING_CODE_DIGITS,
  PAIRING_SECRET_CHARS,
} from '../generated/gateway-constants';

export { PAIR_PARAM };

export { PAIRING_CODE_DIGITS };

const CODE_RE = new RegExp(`^\\d{${PAIRING_CODE_DIGITS}}$`);

const SECRET_RE = new RegExp(`^${DEVICE_PAIRING_SECRET_PREFIX}[a-z2-7]{${PAIRING_SECRET_CHARS}}$`);

export function isPairingCode(value: string): boolean {
  return CODE_RE.test(value);
}

export function isPairingSecret(value: string): boolean {
  return SECRET_RE.test(value);
}

export function pairingCodeToAdopt(search: string, hash = ''): string | null {
  let raw: string | null;
  try {
    const fragment = new URLSearchParams(hash.replace(/^#/, ''));
    raw = (fragment.has(PAIR_PARAM) ? fragment : new URLSearchParams(search)).get(PAIR_PARAM);
  } catch {
    return null;
  }
  const token = raw?.trim() ?? '';
  return isPairingSecret(token) || isPairingCode(token) ? token : null;
}

let taken: string | null | undefined;

export function takePairingCodeFromUrl(): string | null {
  if (taken !== undefined) return taken;
  if (typeof window === 'undefined') {
    taken = null;
    return taken;
  }
  const boot = window as Window & { lucidosPairingSeed?: string | null };
  const url = new URL(window.location.href);
  taken = 'lucidosPairingSeed' in boot
    ? pairingCodeToAdopt('', `#${PAIR_PARAM}=${encodeURIComponent(boot.lucidosPairingSeed ?? '')}`)
    : pairingCodeToAdopt(url.search, url.hash);
  delete boot.lucidosPairingSeed;
  const fragment = new URLSearchParams(url.hash.slice(1));
  if (url.searchParams.has(PAIR_PARAM) || fragment.has(PAIR_PARAM)) {
    url.searchParams.delete(PAIR_PARAM);
    if (fragment.has(PAIR_PARAM)) {
      fragment.delete(PAIR_PARAM);
      url.hash = fragment.toString();
    }
    window.history.replaceState(window.history.state, '', url.toString());
  }
  return taken;
}

export const SPENT_KEY = 'lucidos.pairing.spentLaunchCodes';

const SPENT_LIMIT = 8;

function spentLaunchCodes(): string[] {
  try {
    const raw = window.localStorage.getItem(SPENT_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((c): c is string => typeof c === 'string') : [];
  } catch {
    return [];
  }
}

export function hasUnspentPairingLaunch(): boolean {
  const boot = window as Window & { lucidosPairingLaunch?: boolean };
  if (!boot.lucidosPairingLaunch) return false;
  const code = takePairingCodeFromUrl();
  return !code || !spentLaunchCodes().includes(code);
}

export function releasePairingLaunch(): void {
  (window as Window & { lucidosPairingLaunch?: boolean }).lucidosPairingLaunch = false;
}

function rememberSpentLaunchCode(code: string): void {
  try {
    const next = [code, ...spentLaunchCodes().filter((c) => c !== code)].slice(0, SPENT_LIMIT);
    window.localStorage.setItem(SPENT_KEY, JSON.stringify(next));
  } catch {
  }
}

export function takeUnspentPairingCodeFromUrl(): string | null {
  if (unspent !== undefined) return unspent;
  const code = takePairingCodeFromUrl();
  unspent = !code || spentLaunchCodes().includes(code) ? null : code;
  if (unspent) rememberSpentLaunchCode(unspent);
  return unspent;
}

let unspent: string | null | undefined;

export function resetPairingCodeSeedForTest(): void {
  taken = undefined;
  unspent = undefined;
}
