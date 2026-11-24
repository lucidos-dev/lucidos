
import { isPairingCode, isPairingSecret, pairingCodeToAdopt } from './pairingCodeSeed';

const FORMATTING_RE = /[\s-]/g;

export function pairingCodeFromText(text: string): string | null {
  const raw = text.trim();
  if (!raw) return null;
  if (raw.includes('://')) {
    try {
      const url = new URL(raw);
      return pairingCodeToAdopt(url.search, url.hash);
    } catch {
      return null;
    }
  }
  if (isPairingSecret(raw)) return raw;
  const digits = raw.replace(FORMATTING_RE, '');
  return isPairingCode(digits) ? digits : null;
}
