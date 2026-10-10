/**
 * Read a pairing code out of text the user handed us.
 *
 * Two sources, one rule. The Paste button on the pairing form, and whatever a
 * QR turns out to hold when the camera reads one. Either may carry a pairing
 * URL or the bare digits: the QR encodes the URL, and a person copying a code
 * copies the number.
 *
 * The URL branch is deliberately not `digitsOnly`. Stripping non-digits out of
 * `https://mac.ts.net/~/?pair=01234567` would leave the port and every digit in
 * the hostname glued to the code. So a URL is parsed as a URL, and anything
 * else has to BE a code rather than merely contain one. That strictness is what
 * lets the scanner keep looking instead of accepting a phone number.
 */

import { isPairingCode, pairingCodeToAdopt } from './pairingCodeSeed';

/** Characters people put between digits when they write a code down. Removed
 *  before the grammar is applied, so `4711 8899` pastes as one code. */
const FORMATTING_RE = /[\s-]/g;

/**
 * The pairing code `text` carries, or `null`.
 *
 * Pure, so every rejection is testable without a DOM or a camera.
 */
export function pairingCodeFromText(text: string): string | null {
  const raw = text.trim();
  if (!raw) return null;
  if (raw.includes('://')) {
    try {
      return pairingCodeToAdopt(new URL(raw).search);
    } catch {
      return null;
    }
  }
  const digits = raw.replace(FORMATTING_RE, '');
  return isPairingCode(digits) ? digits : null;
}
