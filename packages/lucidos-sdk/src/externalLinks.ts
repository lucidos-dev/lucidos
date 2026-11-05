// Whether an app page may open an external web page itself.
//
// WebKit gives a tab only to a `window.open` made inside the user's tap. The
// host route (bridge, HTTP, SSE, host `window.open`) carries no activation, so
// Safari blocks it. Where the host would only open a plain browser tab anyway,
// the page opens it directly, synchronously, in the click task.
//
// In a frame the host decides where that holds. Only it knows whether it is the
// desktop app or an installed iOS PWA, and which link target the user chose. It
// pushes the answer on `EXTERNAL_LINKS_CHANNEL` as a frame loads and whenever
// it changes. No answer yet means the host route.
//
// An app opened as its own tab has no host to push. It answers for itself,
// with the same rule minus the link target, which applies only in an iOS PWA.
//
// Every link tap leaves a breadcrumb in the engine log (`traceAppLink`). A
// frame has no log of its own that anyone can read on a phone.

import { onHostPush, tellHost } from './_bridge';
import { inDesktopApp, inIOSStandalone } from './platform';

/** Push channel carrying the host's answer. The host imports this constant
 *  (`store/actions/app-external-links.ts`), so both ends speak one name. */
export const EXTERNAL_LINKS_CHANNEL = 'external-links';

/** Where the host wants a web page opened from: a `tab` the frame opens, the
 *  `safari` app the frame hands off to, or the `host` route. */
export type FrameOpens = 'tab' | 'safari' | 'host';

const FRAME_OPENS: readonly FrameOpens[] = ['tab', 'safari', 'host'];

/** The payload on {@link EXTERNAL_LINKS_CHANNEL}. */
export interface ExternalLinksPush {
  frameOpens: FrameOpens;
}

let frameOpens: FrameOpens = 'host';

/** Take the answer the host pushed. Anything malformed means the host route. */
export function adoptHostExternalLinks(data: unknown): void {
  const given = (data as Partial<ExternalLinksPush> | null)?.frameOpens;
  frameOpens = FRAME_OPENS.find((mode) => mode === given) ?? 'host';
}

/** The answer in force, for the breadcrumb. */
export function currentFrameOpens(): FrameOpens {
  return frameOpens;
}

/** Whether this page should open web pages itself: by the host's answer in a
 *  frame, or as a top-level tab in a plain browser. */
export function pageOpensTabs(): boolean {
  if (typeof window === 'undefined') return false;
  if (window.parent !== window) return frameOpens === 'tab';
  return !inDesktopApp() && !inIOSStandalone();
}

/** Whether the host wants this frame to hand web pages to the Safari app. Only
 *  a frame answers yes: a top-level iOS PWA tab has no host to say so. */
export function pageHandsOffToSafari(): boolean {
  return typeof window !== 'undefined' && window.parent !== window && frameOpens === 'safari';
}

/** iOS's escape hatch from a standalone PWA to the Safari app: a URL with this
 *  prefix goes to the system. The host's `utils/openExternalUrl.ts` imports it.
 *
 *  Temporary measure (undocumented Apple scheme): `docs/temporary-measures.md`
 *  § "`x-safari-` scheme prefix to escape an iOS standalone PWA". */
export const SAFARI_SCHEME_PREFIX = 'x-safari-';

/** Open `url` in a new tab from this page, reporting whether the browser did.
 *  Same rule as the host's `openNewTab` (`utils/newTab.ts`): never the
 *  `noopener` feature, which makes a success return null. Severing `opener`
 *  instead keeps the tab from reaching back into the app. A popup that counts
 *  as cross-origin to this sandboxed frame refuses the write, and the tab is
 *  open all the same. */
export function openTabNow(url: string): boolean {
  const opened = window.open(url, '_blank');
  if (!opened || opened.closed) return false;
  try {
    opened.opener = null;
  } catch {
    // The tab opened; only the severing failed.
  }
  return true;
}

/** Hand `url` to the Safari app from this page, reporting whether the browser
 *  took the open. Same synchronous rule as {@link openTabNow}. */
export function openSafariNow(url: string): boolean {
  return openTabNow(SAFARI_SCHEME_PREFIX + url);
}

/** Longest text a breadcrumb carries for one value. The engine refuses a whole
 *  client log over 4 KB, and a `data:` href can be megabytes. */
const MAX_TRACE_TEXT_LEN = 200;

/** Leave a `[Client/app-link]` breadcrumb in the engine log. Fire-and-forget:
 *  it runs after the tap's own work and never throws into it. */
export function traceAppLink(step: string, detail: Record<string, unknown> = {}): void {
  try {
    const shortened: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(detail)) {
      shortened[key] = typeof value === 'string' && value.length > MAX_TRACE_TEXT_LEN
        ? `${value.slice(0, MAX_TRACE_TEXT_LEN)}…`
        : value;
    }
    tellHost('log.app-link', { step, ...shortened });
  } catch {
    // Telemetry must never break a tap.
  }
}

/** Listen for the host's answer. Returns the teardown. */
export function installExternalLinksListener(): () => void {
  return onHostPush(EXTERNAL_LINKS_CHANNEL, adoptHostExternalLinks);
}
