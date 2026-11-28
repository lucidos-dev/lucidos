
const PAIRING_META = 'meta[name="lucidos-pairing"]';

export function isPairingShellDocument(): boolean {
  if (typeof document === 'undefined') return false;
  return document.querySelector(PAIRING_META) !== null;
}
