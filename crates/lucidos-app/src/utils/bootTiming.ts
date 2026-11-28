import { signal, type ReadonlySignal } from '@preact/signals';
import { parseAppLandingHash } from '../store/actions/appHash';
import { THREAD_HASH_RE } from '../store/actions/threadHash';
import { recordPerfSample } from './perfQueue';

export type BootMilestone = 'clientStarted' | 'connected' | 'threadsLoaded' | 'splashLifted' | 'landed';

export const LANDING_GRACE_MS = 5_000;

const deepLinkLanded = signal(false);
export const bootDeepLinkLanded: ReadonlySignal<boolean> = deepLinkLanded;

const reached: Partial<Record<BootMilestone, number>> = {};
let deepLink = false;
let sent = false;
let graceTimer: ReturnType<typeof setTimeout> | undefined;

export function urlCarriesDeepLink(search: string, hash: string): boolean {
  if (THREAD_HASH_RE.test(hash) || hash === '#notifications' || parseAppLandingHash(hash)) return false;
  const hashNotification = new URLSearchParams(hash.slice(1)).get('notification');
  return Boolean(hashNotification ?? new URLSearchParams(search).get('notification'));
}

export function hasPendingBootLanding(): boolean {
  return deepLink && !sent && !deepLinkLanded.peek();
}

export function markBoot(milestone: BootMilestone): void {
  if (sent || reached[milestone] !== undefined) return;
  if (milestone === 'clientStarted') deepLink = urlCarriesDeepLink(location.search, location.hash);
  else if (reached.clientStarted === undefined) return;
  if (milestone === 'landed' && !deepLink) return;
  reached[milestone] = Math.round(performance.now());
  if (milestone === 'splashLifted') graceTimer = setTimeout(send, LANDING_GRACE_MS);
  if (milestone === 'landed') deepLinkLanded.value = true;
  if (reached.splashLifted !== undefined && reached.landed !== undefined && reached.threadsLoaded !== undefined) {
    send();
  }
}

function send(): void {
  if (sent) return;
  sent = true;
  clearTimeout(graceTimer);
  const nav = performance.getEntriesByType?.('navigation')[0] as PerformanceNavigationTiming | undefined;
  const sample: Record<string, unknown> = {
    deepLink,
    quiet: document.documentElement.hasAttribute('data-boot-splash-quiet'),
    documentMs: nav ? Math.round(nav.responseEnd) : null,
    domReadyMs: nav ? Math.round(nav.domContentLoadedEventEnd) : null,
  };
  for (const [milestone, ms] of Object.entries(reached)) sample[`${milestone}Ms`] = ms;
  recordPerfSample('boot', sample);
}

export function _resetBootTimingForTesting(): void {
  for (const key of Object.keys(reached)) delete reached[key as BootMilestone];
  deepLink = false;
  deepLinkLanded.value = false;
  sent = false;
  clearTimeout(graceTimer);
  graceTimer = undefined;
}
