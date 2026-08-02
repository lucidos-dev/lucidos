import { isIOSPwa } from './platform';
import { currentExternalLinkTarget } from '../store/actions/preferences';

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
 *  Not to be confused with `openExternal` in `utils/tauri.ts`, which is the
 *  desktop app's OS opener (an IPC call into Rust). This is the browser/PWA
 *  side, and `openUrl` picks between them. */
export function openExternalUrl(url: string): void {
  if (!isIOSPwa() || !HTTP_SCHEME_RE.test(url)) {
    window.open(url, '_blank', 'noopener');
    return;
  }
  switch (currentExternalLinkTarget()) {
    case 'in-app':
      window.open(url, '_blank', 'noopener');
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
