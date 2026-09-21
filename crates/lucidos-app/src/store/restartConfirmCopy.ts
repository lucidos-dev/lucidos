import type { CommitGroup, CommitGroupKind, PendingCommits } from '../api/client';
import { GROUP_LABEL, housekeepingLine, pendingCommitsHeadline } from './backgroundActivity';
import type { RestartGroup } from './store';
import type { ConfirmDetails } from './types';

/** The copy behind the restart confirm, and nothing else.
 *
 *  Pure on purpose, beside `progressDialogCopy.ts`. Three surfaces open this
 *  one dialog: the version toast's button, the Lucidos menu's Restart row, and
 *  Settings, System. So the wording lives where none of them owns it.
 *
 *  See docs/plans/2026-09-20-the-switch-says-what-it-brings.md.
 */

/** Everything `showConfirm` needs, as data. */
export interface RestartConfirmCopy {
  title?: string;
  message: string;
  okLabel: string;
  cancelLabel?: string;
  details?: ConfirmDetails;
}

/** How the restart confirm should read, in whichever of its two shapes the
 *  workspace is in.
 *
 *  **A new version** is an offer, so it says what it brings before it asks. The
 *  action keeps its canonical name, *Switch to new version*: the toast, the
 *  glossary and the transcript's own explainer all call it that.
 *
 *  **A plain restart** brings nothing new, so it asks the short question it
 *  always asked and lists the applied changes the restart activates.
 *
 *  `newVersion` is passed in rather than read here, so a test drives both
 *  shapes without a signal. It also makes the caller read the same
 *  `engineNewVersionReady()` the badge and the progress dialog read. */
export function restartConfirmCopy(
  newVersion: boolean,
  pendingCommits: PendingCommits | null,
  restartGroups: RestartGroup[],
): RestartConfirmCopy {
  if (!newVersion) {
    return {
      message: 'Restart engine?',
      okLabel: 'Restart',
      details: appliedChangesDetails(restartGroups),
    };
  }
  return {
    title: 'New version available',
    message: 'The workspace restarts. Threads that are running resume by themselves.',
    okLabel: 'Switch to new version',
    // The same word the toast's secondary action uses, so declining reads the
    // same whichever surface the confirm was opened from.
    cancelLabel: 'Later',
    details: pendingCommitsDetails(pendingCommits) ?? appliedChangesDetails(restartGroups),
  };
}

/** What the switch brings, from the range the engine measured, or `undefined`
 *  when there is no list to draw.
 *
 *  Three inputs land there and none of them may become text. `null` is UNKNOWN,
 *  which a packaged build reports on every poll. A real `total: 0` and an empty
 *  group list are genuine answers, and both mean an empty list. UNKNOWN printed
 *  as a zero is the one outcome forbidden here, so the count is spoken only
 *  from a range that HAS commits in it. */
function pendingCommitsDetails(commits: PendingCommits | null): ConfirmDetails | undefined {
  if (!commits || commits.total === 0 || commits.groups.length === 0) return undefined;
  const housekeeping = commits.groups.find((g) => g.kind === 'housekeeping')?.total ?? 0;
  const described = commits.groups.filter(isDescribed);
  return {
    intro: pendingIntro(commits.total, housekeeping, described.length > 0),
    groups: described.map((group) => ({
      header: GROUP_LABEL[group.kind],
      items: describedItems(group),
    })),
  };
}

/** A group the confirm LISTS, as opposed to the one it counts. A predicate
 *  rather than a bare filter, so `GROUP_LABEL` (which has no housekeeping key)
 *  is indexed without a cast. */
function isDescribed(
  group: CommitGroup,
): group is CommitGroup & { kind: Exclude<CommitGroupKind, 'housekeeping'> } {
  return group.kind !== 'housekeeping';
}

/** The count above the list, with the housekeeping bucket folded into it.
 *
 *  Folded rather than listed, for the reason the toast counts it too: forty doc
 *  commits are not what "what am I getting" means. A headed group with nothing
 *  under it would be worse than a clause, so the clause is where it goes. */
function pendingIntro(total: number, housekeeping: number, described: boolean): string {
  if (housekeeping === 0) return `${pendingCommitsHeadline(total)}.`;
  // Everything in the range is housekeeping, so the count IS the sentence.
  // Saying "12 commits, including 12 housekeeping commits" is true and reads
  // like a mistake.
  if (!described) return `${housekeepingLine(total)} since your running version.`;
  return `${pendingCommitsHeadline(total)}, including ${housekeepingLine(housekeeping)}.`;
}

/** One group's bullets, with the tail the engine capped off named rather than
 *  dropped. A list that silently under-reports its own count is how a reader
 *  concludes a version is smaller than it is. */
function describedItems(group: CommitGroup): string[] {
  const hidden = group.total - group.descriptions.length;
  return hidden > 0 ? [...group.descriptions, `and ${hidden} more`] : [...group.descriptions];
}

/** The applied changes this restart activates, grouped by the thread that
 *  proposed them.
 *
 *  The body a plain restart has always had. It doubles as the fallback for a
 *  new version whose range the engine could not read, which is every packaged
 *  build: those report no commits at all. */
function appliedChangesDetails(groups: RestartGroup[]): ConfirmDetails | undefined {
  if (groups.length === 0) return undefined;
  return {
    intro: 'These changes will be applied:',
    groups: groups.map((g) => ({ header: g.threadTitle, items: g.commits })),
  };
}
