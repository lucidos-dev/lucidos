import type { CommitGroup, CommitGroupKind, PendingCommits } from '../api/client';
import {
  GROUP_LABEL,
  comeWithTheNewVersion,
  describedItems,
  housekeepingLine,
  pendingCommitsHeadline,
} from './backgroundActivity';
import type { RestartGroup } from './store';
import type { ConfirmDetails } from './types';


export interface RestartConfirmCopy {
  title?: string;
  message: string;
  okLabel: string;
  cancelLabel?: string;
  details?: ConfirmDetails;
}

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
    okLabel: 'Switch',
    cancelLabel: 'Later',
    details: pendingCommitsDetails(pendingCommits) ?? appliedChangesDetails(restartGroups),
  };
}

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

function isDescribed(
  group: CommitGroup,
): group is CommitGroup & { kind: Exclude<CommitGroupKind, 'housekeeping'> } {
  return group.kind !== 'housekeeping';
}

function pendingIntro(total: number, housekeeping: number, described: boolean): string {
  if (housekeeping === 0) return `${pendingCommitsHeadline(total)}.`;
  if (!described) return `${housekeepingLine(total)} ${comeWithTheNewVersion(total)}.`;
  return `${pendingCommitsHeadline(total)}, including ${housekeepingLine(housekeeping)}.`;
}

function appliedChangesDetails(groups: RestartGroup[]): ConfirmDetails | undefined {
  if (groups.length === 0) return undefined;
  return {
    intro: 'These changes will be applied:',
    groups: groups.map((g) => ({ header: g.threadTitle, items: g.commits })),
  };
}
