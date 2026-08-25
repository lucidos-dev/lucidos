/**
 * The *What's New badge*: the mark on the whole path into Settings > System >
 * What's New, while something there is waiting to be acted on.
 *
 * Two things raise it. An update available to install, and a release notice
 * still owed an answer. Both are work, which is what the mark promises.
 *
 * Unread release NOTES deliberately do not, though the panel is the same one.
 * That state is true after every upgrade until somebody opens the panel, so it
 * would leave a near-permanent dot on the menu-drawer button. It keeps its own
 * dot on the Lucidos menu's version row instead (see `actions/whatsNew.ts`).
 *
 * The badge is RESOLVABLE: it clears when you install the update or answer the
 * notice, never when you merely see it. So it reads neither
 * `whatsNewSeenRelease` nor `releaseNoticeDismissed`.
 *
 * Both sources are READS beside their signals, never the action modules next
 * to them. A menu-drawer row must not import the update IPC or compose (see
 * `store/releaseNotices.ts`).
 */
import { packagedUpdateVersion } from './packagedUpdate';
import { owedReleaseNoticeCount } from './releaseNotices';

/**
 * What the badge says, or `null` when there is nothing to say.
 *
 * Pure, and the whole of the rule: the presence of the mark IS a non-null
 * sentence, so no surface can draw one without being able to say why.
 *
 * The words live here rather than at each host for the reason
 * `updateControlLabel` exists. Four surfaces owning four spellings is how
 * Settings and What's New once shipped two capitalisations of one button.
 *
 * A middle dot joins the two halves, the separator every multi-part label in
 * the header already uses.
 */
export function whatsNewBadgeLabel(update: string | null, owed: number): string | null {
  const parts: string[] = [];
  if (update) parts.push(`Lucidos ${update} available`);
  if (owed > 0) parts.push(owed === 1 ? '1 thing to do' : `${owed} things to do`);
  return parts.length > 0 ? parts.join(' · ') : null;
}

/**
 * The live answer, read off the signals, or `null` for no badge.
 *
 * It reads them at call time, so calling it during render subscribes the
 * caller. Neither source needs a fetch of its own here: the notices load at
 * startup and refresh on their SSE arm, and the release check arrives with the
 * status poll.
 *
 * An unknown answer is not "you owe something". `owedReleaseNoticeCount`
 * answers `0` for every state but `loaded`. That is what keeps a cold load, or
 * a dead engine, from flashing a dot that clears itself.
 */
export function whatsNewBadge(): string | null {
  return whatsNewBadgeLabel(packagedUpdateVersion(), owedReleaseNoticeCount());
}
