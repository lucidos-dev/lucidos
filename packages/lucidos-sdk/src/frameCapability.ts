/**
 * The URL pass this app frame carries to its own workspace files.
 *
 * An app frame runs at an opaque origin (ADR 0227), so the browser sends no
 * device credential with any subresource of its document. Behind a gateway
 * that refuses the app's own `style.css` and everything `data.url()` builds.
 * The engine mints a short-lived pass instead and stamps it into the document's
 * `<base href>` (ADR 0238).
 *
 * The base is the single source of truth, deliberately. The browser resolves
 * the app's own relative refs against it. This module reads the same value for
 * the URLs the SDK builds, so the two can never disagree.
 *
 * A direct-to-engine hit has no pass, because it has no device gate to pass.
 * Every function here answers "no capability" there, and every URL comes out
 * exactly as it did before.
 */

import { onHostPush } from './_bridge';

/** The path segment that introduces a pass. Mirrors `lucidos-frame-capability`. */
export const CAPABILITY_SEGMENT = '~cap';

/** The bridge channel the host re-mints on. */
export const CAPABILITY_CHANNEL = 'frame-capability';

export interface SplitPath {
  /** Everything before the segment: `/<slug>`. */
  prefix: string;
  /** The opaque token itself. */
  capability: string;
  /** Everything after it, keeping its leading slash. */
  rest: string;
}

/** Take a path apart at its `/~cap/<token>/` segment, or `null` if it has none. */
export function splitCapability(path: string): SplitPath | null {
  const marker = `/${CAPABILITY_SEGMENT}/`;
  const at = path.indexOf(marker);
  if (at < 0) return null;
  const after = path.slice(at + marker.length);
  const slash = after.indexOf('/');
  if (slash <= 0) return null;
  return { prefix: path.slice(0, at), capability: after.slice(0, slash), rest: after.slice(slash) };
}

/** The `<base>` element the engine stamped, if this document has one. */
function baseElement(): HTMLBaseElement | null {
  if (typeof document === 'undefined') return null;
  return document.querySelector('base');
}

/**
 * The pass this frame holds right now, or `null`.
 *
 * Read fresh on every call rather than cached at load. A renewal replaces the
 * value in place, and a cache would hand out the lapsed one.
 */
export function currentCapability(): string | null {
  const href = baseElement()?.getAttribute('href');
  if (!href) return null;
  return splitCapability(href)?.capability ?? null;
}

/**
 * The `/~cap/<token>` a URL builder should splice in, or `''`.
 *
 * `''` covers both the direct-to-engine case and a document the engine seeded
 * with nothing, so a caller needs no branch of its own.
 */
export function capabilityCarrier(): string {
  const capability = currentCapability();
  return capability ? `/${CAPABILITY_SEGMENT}/${capability}` : '';
}

/**
 * Swap a freshly minted pass into the `<base href>`.
 *
 * This is what keeps a long-open app working. The host re-mints at half-life
 * and pushes the new token down the bridge. One attribute write then carries it
 * to every ref the DOCUMENT resolves afterwards. Nothing reloads, and no
 * already-loaded resource is disturbed.
 *
 * It does not reach a url captured at load time: a dynamic `import()` inside an
 * ES module resolves against that module's own url, and a stylesheet's `url()`
 * against the stylesheet's. Each keeps the pass it loaded with, so a lazy load
 * an hour on is refused. ADR 0238 § Consequences carries the class.
 *
 * `history.replaceState` would move the document's URL the same way, and it
 * throws at an opaque origin on WebKit. A `<base>` write is plain DOM.
 *
 * Returns false when there is nothing to renew, which is every document the
 * engine stamped no pass into.
 */
export function adoptCapability(capability: string): boolean {
  if (!capability || capability.includes('/')) return false;
  const element = baseElement();
  const href = element?.getAttribute('href');
  if (!element || !href) return false;
  const split = splitCapability(href);
  if (!split) return false;
  element.setAttribute(
    'href',
    `${split.prefix}/${CAPABILITY_SEGMENT}/${capability}${split.rest}`,
  );
  return true;
}

/** What the host fans out on the `frame-capability` channel. */
interface RenewedCapability {
  capability?: unknown;
}

/**
 * Listen for the renewal the host pushes, and take it.
 *
 * The host owns the schedule, because it is the side holding the device
 * credential and a real origin. An app asks for nothing and notices nothing.
 *
 * Returns the unsubscribe, for symmetry with the other bridge listeners. The
 * frame outlives it in practice, so nothing calls it.
 */
export function installCapabilityRenewal(): () => void {
  return onHostPush(CAPABILITY_CHANNEL, (payload) => {
    const renewed = (payload ?? {}) as RenewedCapability;
    if (typeof renewed.capability === 'string') adoptCapability(renewed.capability);
  });
}
