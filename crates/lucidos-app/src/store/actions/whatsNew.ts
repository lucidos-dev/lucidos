/**
 * Settings > System > What's New: loading the changelog, and the unread dot on
 * the Lucidos menu's version row.
 *
 * The dot answers one question, "is there a release I have not read the notes
 * for", and it is deliberately a CLIENT fact rather than an account one: the
 * same workspace opened on a laptop and a phone should each get told once.
 */
import { changelogReleases, whatsNewSeenRelease } from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import { engineChangelog } from '../../api/client';

/** localStorage key for {@link whatsNewSeenRelease}. */
const SEEN_RELEASE_KEY = 'lucidos-whats-new-seen-release';

export async function loadChangelog(): Promise<void> {
  setLoadingIfFresh(changelogReleases);
  try {
    changelogReleases.value = { status: 'loaded', data: await engineChangelog() };
  } catch (error) {
    changelogReleases.value = toFailed(error);
  }
}

/**
 * Is there a release whose notes this client has not opened?
 *
 * Pure, and the whole of the rule. Two of the four cases are the ones that bite:
 *
 * - `release === null` is the window before /health answers, and it is NOT
 *   "nothing new". Answering true there would flash a dot on every reload;
 *   answering it by writing the null through {@link markWhatsNewSeen} would
 *   record "I have read release null" and suppress the dot forever after.
 * - `seen === null` is a client that has never opened the panel, including a
 *   brand new install. That DOES get a dot: one, pointing at what is in the
 *   version they just installed, and it clears itself on the first open.
 */
export function hasUnreadWhatsNew(release: string | null, seen: string | null): boolean {
  if (!release) return false;
  return seen !== release;
}

/**
 * Record that this client has read the notes for `release`.
 *
 * Ignores a null release for the reason above: the panel can be opened while
 * /health is still in flight, and recording an unknown release as read would
 * spend the one notification the user gets for it.
 */
export function markWhatsNewSeen(release: string | null): void {
  if (!release || whatsNewSeenRelease.value === release) return;
  whatsNewSeenRelease.value = release;
  localStorage.setItem(SEEN_RELEASE_KEY, release);
}
