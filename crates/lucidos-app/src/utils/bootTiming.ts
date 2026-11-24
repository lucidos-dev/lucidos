import { recordPerfSample } from './perfQueue';

export type BootMilestone = 'clientStarted' | 'connected' | 'threadsLoaded' | 'splashLifted' | 'landed';

export const LANDING_GRACE_MS = 5_000;

const reached: Partial<Record<BootMilestone, number>> = {};
let deepLink = false;
let sent = false;
let graceTimer: ReturnType<typeof setTimeout> | undefined;

export function urlCarriesDeepLink(search: string, hash: string): boolean {
  return /[?#&]notification=[^&#]/.test(search + hash);
}

export function markBoot(milestone: BootMilestone): void {
  if (sent || reached[milestone] !== undefined) return;
  if (milestone === 'clientStarted') deepLink = urlCarriesDeepLink(location.search, location.hash);
  else if (reached.clientStarted === undefined) return;
  if (milestone === 'landed' && !deepLink) return;
  reached[milestone] = Math.round(performance.now());
  if (milestone === 'splashLifted') graceTimer = setTimeout(send, LANDING_GRACE_MS);
  if (reached.splashLifted !== undefined && reached.landed !== undefined) send();
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
  sent = false;
  clearTimeout(graceTimer);
  graceTimer = undefined;
}
