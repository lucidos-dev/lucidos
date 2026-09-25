/**
 * The slowness warning, client side (ADRs 0274, 0283).
 *
 * The gateway owns the measurement and serves a cached answer. This polls it
 * while the window is visible, and on resume, and remembers which episode the
 * user dismissed. The dismissal is per device and safe to lose, so it lives in
 * `localStorage` rather than in a preference.
 */
import { signal } from '@preact/signals';
import { getSlownessStatus, type SlownessStatus } from '../../api/client/control';

export type SlownessEpisode = Extract<SlownessStatus, { state: 'slow' }>;

export const SLOWNESS_POLL_MS = 60_000;
const DISMISSED_KEY = 'slowness-dismissed-episode';

export const slownessStatus = signal<SlownessStatus | null>(null);
export const dismissedSlownessEpisode = signal<string | null>(readDismissed());

function readDismissed(): string | null {
  try {
    return localStorage.getItem(DISMISSED_KEY);
  } catch {
    return null;
  }
}

/** The episode this window shows: an open one the user has not dismissed.
 *
 *  Memory belongs to the whole computer, so every workspace shows it. An
 *  unclear slowdown is only true of the workspaces that were slow, so a fast
 *  one never claims it. `workspaceId` is `null` outside a workspace. A reason
 *  this build does not know shows nothing. */
export function visibleSlownessEpisode(
  status: SlownessStatus | null,
  dismissed: string | null,
  workspaceId: string | null,
): SlownessEpisode | null {
  if (status?.state !== 'slow' || status.episode_id === dismissed) return null;
  switch (status.reason) {
    case 'memory':
      return status;
    case 'unclear':
      return workspaceId !== null && status.slow_workspaces.includes(workspaceId) ? status : null;
    default:
      return null;
  }
}

/** Hide this episode on this device. The next one shows again. */
export function dismissSlownessEpisode(episodeId: string): void {
  dismissedSlownessEpisode.value = episodeId;
  try {
    localStorage.setItem(DISMISSED_KEY, episodeId);
  } catch {
    // Private browsing: the dismissal still holds for this page's life.
  }
}

let lastRefreshFailed = false;

/** Read the gateway's answer. With no gateway (a direct engine port) or an
 *  older one without the route, the banner stays away rather than showing a
 *  stale episode. */
export async function refreshSlowness(): Promise<void> {
  try {
    slownessStatus.value = await getSlownessStatus();
    lastRefreshFailed = false;
  } catch (e) {
    slownessStatus.value = null;
    // No toast: the user did not ask for this, and a warning that cannot be
    // measured is simply absent (ADR 0274). The next poll retries on its own.
    // Warn once per failing streak, not once a minute.
    if (!lastRefreshFailed) {
      console.warn('[slowness] gateway status unavailable; retrying each poll', e);
    }
    lastRefreshFailed = true;
  }
}

let pollTimer: ReturnType<typeof setInterval> | null = null;

/** Poll while the window is visible. A hidden window skips the tick, and the
 *  resume path reads again on return. */
export function startSlownessChecks(): void {
  if (pollTimer !== null) return;
  void refreshSlowness();
  pollTimer = setInterval(() => {
    if (document.visibilityState === 'visible') void refreshSlowness();
  }, SLOWNESS_POLL_MS);
}

export function stopSlownessChecks(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}
