/**
 * Is this document the pairing screen?
 *
 * The gateway answers an unauthenticated navigation with the pairing screen AT
 * the url asked for (ADR 0094), and that screen is the picker document. So it
 * carries the picker's `<base href="/~/">`, and anything keying on that base
 * alone cannot tell the two apart.
 *
 * The response header `x-lucidos-pairing` marks it for the service worker. This
 * marker is the same fact in the DOM, for script (`server.rs`
 * `PAIRING_SHELL_META`). It is stamped beside the base href, so it is parsed
 * before the inline scripts in `<head>` and before this bundle.
 *
 * What it stands down is auto-open. On the pairing screen every route into a
 * workspace leads back here, so taking one is a navigation loop rather than a
 * shortcut. The cold-start fast path in `index.html` repeats this check by hand,
 * because it runs before any module.
 */

/** Name of the `<meta>` the gateway stamps into the pairing document. */
const PAIRING_META = 'meta[name="lucidos-pairing"]';

/** True when the gateway served this document as the pairing screen. */
export function isPairingShellDocument(): boolean {
  if (typeof document === 'undefined') return false;
  return document.querySelector(PAIRING_META) !== null;
}
