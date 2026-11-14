/** One `[Client/perf] boot` sample per document: when each boot milestone was
 *  first reached, in ms since navigation start.
 *
 *  An iOS push tap reloads the whole document, so this sample is the tap's cost
 *  step by step, from the reload to the landed thread. Read it before trying to
 *  make a tap faster: it names the step that costs the most.
 *
 *  Rides the perf queue, so it records nothing unless perf recording is on
 *  (Settings → System → Debugging). Best-effort telemetry: it reports on a boot
 *  rather than being part of one, and nothing waits on it. */
import { recordPerfSample } from './perfQueue';

/** The milestones in the order a push tap meets them. */
export type BootMilestone = 'clientStarted' | 'connected' | 'threadsLoaded' | 'splashLifted' | 'landed';

/** How long after the splash lifts the sample waits for a landing. A plain
 *  launch, a refresh and a tap that opens a panel never land, so the sample
 *  goes out without one. */
export const LANDING_GRACE_MS = 5_000;

const reached: Partial<Record<BootMilestone, number>> = {};
let deepLink = false;
let sent = false;
let graceTimer: ReturnType<typeof setTimeout> | undefined;

/** Does this URL carry a notification deep link? Read once, before the router
 *  consumes it, so a later in-app landing is never taken for the tap's. */
export function urlCarriesDeepLink(search: string, hash: string): boolean {
  return /[?#&]notification=[^&#]/.test(search + hash);
}

/** Record that a milestone was reached. Only the first call per milestone
 *  counts, and nothing counts once the sample has gone out. Nothing counts
 *  before `clientStarted` either: the picker lifts a splash too, but it boots
 *  no workspace client, so it has no boot to report. */
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
    // The document's own share: its HTML arriving, then the parse finishing.
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
