/**
 * The *System attention badge*: the mark on the whole path into Settings >
 * System, while something there is waiting to be acted on.
 *
 * Two things raise it, on two different pages. An update to install is What's
 * New. A notice owed an answer is Release Notices. Both are work.
 *
 * Unread release NOTES deliberately do not. That state is true after every
 * upgrade until somebody opens the panel, so it would leave a near-permanent
 * dot on the menu-drawer button. It keeps its own dot on the Lucidos menu's
 * version row instead (see `actions/whatsNew.ts`).
 *
 * The badge is RESOLVABLE: it clears when you install the update or answer the
 * notice, never when you merely see it. So it reads neither
 * `whatsNewSeenRelease` nor `releaseNoticeDismissed`.
 *
 * Both sources are READS beside their signals, never the action modules next
 * to them (see `store/releaseNotices.ts`).
 */
import { packagedUpdateVersion } from './packagedUpdate';
import { owedReleaseNoticeCount } from './releaseNotices';
import type { SettingsNavKey } from './store';

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
export function systemAttentionBadgeLabel(update: string | null, owed: number): string | null {
  const parts: string[] = [];
  if (update) parts.push(`Lucidos ${update} available`);
  if (owed > 0) parts.push(owed === 1 ? '1 thing to do' : `${owed} things to do`);
  return parts.length > 0 ? parts.join(' · ') : null;
}

/**
 * The live answer for the whole path, read off the signals, or `null`.
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
export function systemAttentionBadge(): string | null {
  return systemAttentionBadgeLabel(packagedUpdateVersion(), owedReleaseNoticeCount());
}

/**
 * The two halves, for the two PAGES the causes now live on.
 *
 * A badge on a page's own row promises work on THAT page, so each reads only
 * its own source. The union above is for everything upstream of the split,
 * where one mark stands for both destinations.
 *
 * Built from the same {@link systemAttentionBadgeLabel}, so the surfaces cannot
 * end up with four spellings of one sentence.
 */
export function updateBadge(): string | null {
  return systemAttentionBadgeLabel(packagedUpdateVersion(), 0);
}

/** The owed-notice half. See {@link updateBadge}. */
export function releaseNoticeBadge(): string | null {
  return systemAttentionBadgeLabel(null, owedReleaseNoticeCount());
}

/**
 * What a System sub-page owes, by its subview key, or `null` for a page that
 * can owe nothing.
 *
 * The routing rule in one place, rather than a pair of `key ===` tests in the
 * submenu. Every other page answers null, so a new sub-page is unmarked until
 * somebody names a source for it here.
 *
 * A `SettingsNavKey`, never a bare string. `SystemPage` spells this same page
 * `overview` in its own `SystemPanel` type, and that spelling passed here would
 * compile and silently answer null for good.
 */
export function systemPageBadge(key: SettingsNavKey): string | null {
  if (key === 'whats-new') return updateBadge();
  if (key === 'release-notices') return releaseNoticeBadge();
  return null;
}
