import { isIOSPwa } from './platform';
import { openNewTab } from './newTab';
import { currentExternalLinkTarget } from '../store/actions/preferences';
import { postClientLog } from './clientLog';
import { showToast, dismissToast } from '../store/store';

/** iOS's only escape hatch from a standalone PWA to the real Safari app.
 *  Navigating the document to `x-safari-https://…` hands the URL to Safari;
 *  WebKit treats the prefixed string as a scheme it doesn't own and passes it to
 *  the system.
 *
 *  Temporary measure (undocumented Apple scheme, workaround for a WebKit gap):
 *  `docs/temporary-measures.md` § "`x-safari-` scheme prefix to escape an iOS
 *  standalone PWA" carries the removal condition. */
const SAFARI_SCHEME_PREFIX = 'x-safari-';

/** Only http(s) is ever rewritten. `mailto:`, `tel:`, `file:` and `data:` have
 *  their own handlers and must reach them untouched, in every mode. */
const HTTP_SCHEME_RE = /^https?:\/\//i;

/** Hand `url` to the Safari app. The `safari` mode, and the fallback every
 *  other iOS mode degrades to. */
function handOffToSafari(url: string): void {
  window.location.href = SAFARI_SCHEME_PREFIX + url;
}

/** Longest URL text carried into a toast or a breadcrumb. A `data:` URL reaches
 *  here intact and can be megabytes: the engine answers 400 over a 4KB payload
 *  (api/internal.rs) and would lose the whole line, and a toast is one strip of
 *  chrome. The recovery button carries the FULL url, so nothing is truncated
 *  except the text. */
const MAX_URL_TEXT_LEN = 200;

function shortUrl(url: string): string {
  return url.length <= MAX_URL_TEXT_LEN ? url : `${url.slice(0, MAX_URL_TEXT_LEN)}…`;
}

/** Toast key for a blocked open. Per URL, so a repeated navigate to the SAME
 *  page refreshes one toast instead of stacking, while two different blocked
 *  URLs stay two offers: collapsing them onto one key would drop the first URL,
 *  which is the loss this whole path exists to prevent. */
function blockedKey(url: string): string {
  return `url-blocked-${url}`;
}

/** Open a new tab, and when the browser refuses, keep the URL reachable instead
 *  of dropping it.
 *
 *  The refusal is the whole reason this wrapper exists. `openUrl` is reached
 *  from an SSE handler (`handleNavigationRequest` → `case 'url'`) as well as
 *  from clicks, and a network event carries no transient user activation, so
 *  Chrome blocks the popup. Discarding the return value made that a total
 *  silence: no tab, nothing in any log, no toast, while the engine had already
 *  told the agent the device was asked to open the page, so the agent sent the
 *  user to look at a page that never appeared.
 *
 *  Two outputs, because the two audiences differ. A `[Client/nav]` breadcrumb
 *  makes it diagnosable after the fact from engine.log, and a toast gives the
 *  user the page back: its Open button retries from inside a real click, which
 *  DOES carry activation, so the retry goes through. Never auto-dismissed, since
 *  the URL is gone the moment the toast is.
 *
 *  `source` is where the navigate came from (a thread label, or "an app"). Both
 *  outputs carry it, because nothing the user did produced this toast and a
 *  message that names only the URL leaves them with no idea what asked for it
 *  (`.claude/rules/frontend.md` § No Hidden Errors). Absent for a direct click,
 *  which needs no explaining. */
function openNewTabOrOffer(url: string, source?: string): void {
  if (openNewTab(url)) return;
  logBlocked('external-url-blocked', url, source);
  offerBlockedUrl(url, source, false);
}

function logBlocked(message: string, url: string, source: string | undefined): void {
  postClientLog('nav', message, { url: shortUrl(url), source: source ?? null });
}

/** Show (or refresh) the recovery offer for a URL the browser refused to open.
 *  `retried` distinguishes the first block from a retry that was ALSO blocked:
 *  a click carries activation, so a second refusal means pop-ups are off for
 *  this site entirely and the user has to change that themselves. Saying so is
 *  the difference between a dead button and an actionable one. */
function offerBlockedUrl(url: string, source: string | undefined, retried: boolean): void {
  const key = blockedKey(url);
  const shown = shortUrl(url);
  const from = source ? ` (requested by ${source})` : '';
  showToast(
    retried
      ? `Still blocked${from}. Allow pop-ups for this site to open ${shown}`
      : `Your browser blocked opening ${shown}${from}`,
    'warning',
    { key, action: { label: 'Open', onClick: () => retryBlockedUrl(url, source) } },
  );
}

function retryBlockedUrl(url: string, source: string | undefined): void {
  if (openNewTab(url)) {
    // Log the recovery too, so engine.log answers "did the user ever get
    // there?" rather than only "something was blocked" (same matched-pair shape
    // as the `[Client/ipc]` failing/recovered lines in utils/ipcHealth).
    logBlocked('external-url-opened-on-retry', url, source);
    dismissToast(blockedKey(url));
    return;
  }
  logBlocked('external-url-still-blocked', url, source);
  offerBlockedUrl(url, source, true);
}

/** Open a URL OUTSIDE the Lucidos app: a new browser tab everywhere, and on an
 *  installed iOS PWA whichever of three targets the user chose.
 *
 *  WHY iOS needs its own branch at all, so nobody "simplifies" this back to a
 *  bare `window.open`: inside an iOS standalone PWA (`display-mode: standalone`),
 *  neither `window.open(url, '_blank')` nor an `<a target="_blank">` escapes the
 *  app. WebKit renders both in the PWA's own in-app web view overlay, which has
 *  no address bar, no tabs, no shared Safari session, and no way back to a real
 *  browser.
 *
 *  The three modes (`store/actions/preferences.ts` § external link target):
 *
 *  - `safari` (default) navigates the top-level document to the `x-safari-`
 *    prefixed URL. iOS backgrounds the PWA and foregrounds Safari, leaving the
 *    PWA exactly where it was. Because the app is backgrounded rather than
 *    navigated there is nothing to fall back FROM, so no fallback timer.
 *  - `ask` opens the OS share sheet. This is the ONLY mode where iOS itself
 *    decides: the sheet lists every installed browser, including the user's real
 *    default, which no web API lets us either read or target directly.
 *  - `in-app` is the pre-2026-08 behaviour, kept because some users genuinely
 *    want links to stay inside the app.
 *
 *  The two routes that go through `window.open` (the ordinary browser case and
 *  `in-app`) report a blocked open rather than swallowing it, see
 *  `openNewTabOrOffer`. `safari` and `ask` cannot be blocked, since neither
 *  opens a window.
 *
 *  `source` names where a non-click navigate originated (a thread label, or
 *  "an app"), and is used only to attribute a blocked-open toast.
 *
 *  Not to be confused with `openExternal` in `utils/tauri.ts`, which is the
 *  desktop app's OS opener (an IPC call into Rust). This is the browser/PWA
 *  side, and `openUrl` picks between them. */
export function openExternalUrl(url: string, source?: string): void {
  if (!isIOSPwa() || !HTTP_SCHEME_RE.test(url)) {
    openNewTabOrOffer(url, source);
    return;
  }
  switch (currentExternalLinkTarget()) {
    case 'in-app':
      openNewTabOrOffer(url, source);
      return;
    case 'ask':
      shareOrHandOff(url);
      return;
    // `safari` and anything a future build adds. The default is NOT dead code
    // standing in for exhaustiveness: this function returns void, so TypeScript
    // cannot flag an unhandled member, and a switch that matched nothing would
    // open nothing at all. A new mode arriving before its branch does must
    // degrade to the working hand-off, never to a dead tap.
    case 'safari':
    default:
      handOffToSafari(url);
      return;
  }
}

/** `ask` mode. Must be called synchronously from the user's gesture: the Web
 *  Share API requires transient activation, and an `await` before `share()`
 *  spends it.
 *
 *  Never dead-ends. No `navigator.share` at all (an older iOS, or a stripped
 *  environment) falls straight through to the Safari hand-off; so does any
 *  rejection EXCEPT the user's own dismissal of the sheet. `AbortError` means
 *  they looked at the options and chose none, so re-routing them to Safari would
 *  override the answer they just gave. */
function shareOrHandOff(url: string): void {
  if (typeof navigator.share !== 'function') {
    handOffToSafari(url);
    return;
  }
  navigator.share({ url }).catch((err: unknown) => {
    if (err instanceof Error && err.name === 'AbortError') return;
    handOffToSafari(url);
  });
}
