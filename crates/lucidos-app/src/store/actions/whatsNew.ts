import { changelogReleases, whatsNewSeenRelease } from '../store';
import { failedIfFresh, setLoadingIfFresh } from '../types';
import { engineChangelog } from '../../api/client';

const SEEN_RELEASE_KEY = 'lucidos-whats-new-seen-release';

export async function loadChangelog(): Promise<void> {
  setLoadingIfFresh(changelogReleases);
  try {
    changelogReleases.value = { status: 'loaded', data: await engineChangelog() };
  } catch (error) {
    changelogReleases.value = failedIfFresh(changelogReleases.peek(), error);
  }
}

export async function reloadChangelogIfRead(): Promise<void> {
  if (changelogReleases.peek().status === 'not-loaded') return;
  await loadChangelog();
}

export function hasUnreadWhatsNew(release: string | null, seen: string | null): boolean {
  if (!release) return false;
  return seen !== release;
}

export function markWhatsNewSeen(release: string | null): void {
  if (!release || whatsNewSeenRelease.value === release) return;
  whatsNewSeenRelease.value = release;
  localStorage.setItem(SEEN_RELEASE_KEY, release);
}
