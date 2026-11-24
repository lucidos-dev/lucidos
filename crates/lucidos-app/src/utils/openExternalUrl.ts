import { isIOSPwa } from './platform';
import { openNewTab } from './newTab';
import { hasDangerousScheme } from './dangerousUrlScheme';
import { currentExternalLinkTarget } from '../store/actions/preferences';
import { postClientLog } from './clientLog';
import { showToast, dismissToast } from '../store/store';
import { SAFARI_SCHEME_PREFIX } from '../../../../packages/lucidos-sdk/src/externalLinks';

const HTTP_SCHEME_RE = /^https?:\/\//i;

function handOffToSafari(url: string): void {
  window.location.href = SAFARI_SCHEME_PREFIX + url;
}

const MAX_URL_TEXT_LEN = 200;

function shortUrl(url: string): string {
  return url.length <= MAX_URL_TEXT_LEN ? url : `${url.slice(0, MAX_URL_TEXT_LEN)}…`;
}

function blockedKey(url: string): string {
  return `url-blocked-${url}`;
}

function openNewTabOrOffer(url: string, source?: string): void {
  if (openNewTab(url)) return;
  logBlocked('external-url-blocked', url, source);
  offerBlockedUrl(url, source, false);
}

function logBlocked(message: string, url: string, source: string | undefined): void {
  postClientLog('nav', message, { url: shortUrl(url), source: source ?? null });
}

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

export function refuseDangerousUrl(url: string, source?: string): boolean {
  if (!hasDangerousScheme(url)) return false;
  const from = source ? ` (requested by ${source})` : '';
  showToast(`Refused to open a scripted url${from}`, 'error');
  return true;
}

function retryBlockedUrl(url: string, source: string | undefined): void {
  if (openNewTab(url)) {
    logBlocked('external-url-opened-on-retry', url, source);
    dismissToast(blockedKey(url));
    return;
  }
  logBlocked('external-url-still-blocked', url, source);
  offerBlockedUrl(url, source, true);
}

export function openExternalUrl(url: string, source?: string): void {
  if (refuseDangerousUrl(url, source)) return;
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
    case 'safari':
    default:
      handOffToSafari(url);
      return;
  }
}

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
