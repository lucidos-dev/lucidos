import { lucidos } from './index';
import { isBridged } from './_bridge';
import { currentFrameOpens, openSafariNow, openTabNow, traceAppLink } from './externalLinks';
import { externalWebUrl } from './webLink';

export function installAppLinkRouting(): void {
  document.addEventListener('click', (e: MouseEvent) => {
    const target = e.target as Element | null;
    const anchor = target?.closest?.('a[href]') as HTMLAnchorElement | null;
    if (!anchor) return;
    const href = anchor.getAttribute('href');
    if (!href) return;
    const webUrl = externalWebUrl(href);
    if (isBridged() && !webUrl && anchor.hasAttribute('download') && !/^[a-z][a-z0-9+.-]*:/i.test(href)) {
      e.preventDefault();
      e.stopPropagation();
      const url = new URL(href, document.baseURI);
      url.searchParams.set('download', '1');
      const via = document.createElement('a');
      via.href = url.href;
      via.download = anchor.getAttribute('download') || '';
      via.rel = 'noopener';
      document.body.appendChild(via);
      via.click();
      setTimeout(() => via.remove(), 0);
      traceAppLink('route', { route: 'download', href });
      return;
    }
    if (webUrl) {
      e.preventDefault();
      e.stopPropagation();
      lucidos.ui.openExternal(webUrl).catch((err) => {
        console.warn('[lucidos-sdk] external link fell back to window.open:', webUrl, err);
        traceAppLink('host-route-rejected', { href: webUrl, error: String(err) });
        const openNow = currentFrameOpens() === 'safari' ? openSafariNow : openTabNow;
        if (!openNow(webUrl)) {
          lucidos.ui.toast(`Could not open ${webUrl}. Try the link again.`, 'error');
          traceAppLink('toast', { href: webUrl });
        }
      });
      traceAppLink('route', { route: 'external', href: webUrl, opens: currentFrameOpens() });
    } else if (anchor.getAttribute('target') === '_blank') {
      e.preventDefault();
      e.stopPropagation();
      const resolved = new URL(href, window.location.href).pathname;
      window.location.href = resolved;
      traceAppLink('route', { route: 'same-site-blank', href });
    }
  }, true);
}
