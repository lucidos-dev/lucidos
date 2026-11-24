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

export function visibleSlownessEpisode(
  status: SlownessStatus | null,
  dismissed: string | null,
  workspaceId: string | null,
): SlownessEpisode | null {
  if (status?.state !== 'slow' || status.episode_id === dismissed) return null;
  switch (status.reason) {
    case 'disk':
    case 'memory':
      return status;
    case 'database':
    case 'unclear':
      return workspaceId !== null && status.slow_workspaces.includes(workspaceId) ? status : null;
    default:
      return null;
  }
}

export function dismissSlownessEpisode(episodeId: string): void {
  dismissedSlownessEpisode.value = episodeId;
  try {
    localStorage.setItem(DISMISSED_KEY, episodeId);
  } catch {
  }
}

let lastRefreshFailed = false;

export async function refreshSlowness(): Promise<void> {
  try {
    slownessStatus.value = await getSlownessStatus();
    lastRefreshFailed = false;
  } catch (e) {
    slownessStatus.value = null;
    if (!lastRefreshFailed) {
      console.warn('[slowness] gateway status unavailable; retrying each poll', e);
    }
    lastRefreshFailed = true;
  }
}

let pollTimer: ReturnType<typeof setInterval> | null = null;

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
