import { lucidos } from './index';
import { isBridged } from './_bridge';
import { externalWebUrl } from './webLink';

/** Route a click on a link inside an app frame. Installed in the capture phase,
 *  after the link press guard, so a swipe never reaches it. */
export function installAppLinkRouting(): void {
  document.addEventListener('click', (e: MouseEvent) => {
    const target = e.target as Element | null;
    const anchor = target?.closest?.('a[href]') as HTMLAnchorElement | null;
    if (!anchor) return;
    const href = anchor.getAttribute('href');
    if (!href) return;
    // Scheme case and a protocol-relative `//host` make no difference: such a
    // link leaves the frame, so it goes to the host and never to the browser's
    // own navigation.
    const webUrl = externalWebUrl(href);
    // A download of one of the app's OWN files. The frame is isolated, so that
    // file is cross-origin to it and the browser ignores the `download`
    // attribute: the click would navigate the frame to the file. Ask the engine
    // for it as an attachment instead, which is what a browser does obey.
    // `blob:` and `data:` need none of this and are left alone.
    if (isBridged() && !webUrl && anchor.hasAttribute('download') && !/^[a-z][a-z0-9+.-]*:/i.test(href)) {
      e.preventDefault();
      e.stopPropagation();
      // Against `document.baseURI`, which is what the browser itself resolves
      // this href against. Behind a gateway the base carries the frame
      // capability (ADR 0238). `window.location.href` does not, so the older
      // form asked for a file with no pass on it.
      const url = new URL(href, document.baseURI);
      url.searchParams.set('download', '1');
      const via = document.createElement('a');
      via.href = url.href;
      via.download = anchor.getAttribute('download') || '';
      via.rel = 'noopener';
      document.body.appendChild(via);
      via.click();
      // Removed on a later task, never in the same one. A download starts
      // asynchronously, and tearing the anchor out from under it is enough to
      // lose it on some engines.
      setTimeout(() => via.remove(), 0);
      return;
    }
    if (webUrl) {
      e.preventDefault();
      e.stopPropagation();
      // openExternal, not navigate: it honours the user's external-link
      // preference and, in "Ask" mode, must run inside this frame while the
      // click's user activation is still live. Called synchronously here for
      // exactly that reason.
      //
      // The window.open fallback stays. It fires only after the host route
      // failed (engine unreachable). No path can honour the preference then.
      // The choice is the in-app web view or a tap that does nothing, and a
      // visible page wins.
      lucidos.ui.openExternal(webUrl).catch((err) => {
        console.warn('[lucidos-sdk] external link fell back to window.open:', webUrl, err);
        window.open(webUrl, '_blank', 'noopener');
      });
    } else if (anchor.getAttribute('target') === '_blank') {
      e.preventDefault();
      e.stopPropagation();
      const resolved = new URL(href, window.location.href).pathname;
      window.location.href = resolved;
    }
  }, true);
}
