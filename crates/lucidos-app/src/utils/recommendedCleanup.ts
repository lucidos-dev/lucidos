import { formatBytes } from './formatBytes';

export function countOf(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? '' : 's'}`;
}

export interface RecommendedCleanupOutcome {
  removedCount: number;
  cleanedCount: number;
  freedBytes: number;
}

/** The toast for a finished recommended cleanup. */
export function describeRecommendedCleanupOutcome({
  removedCount,
  cleanedCount,
  freedBytes,
}: RecommendedCleanupOutcome): string {
  const done = [
    removedCount > 0 && `removed ${countOf(removedCount, 'finished worktree')}`,
    cleanedCount > 0 && `cleared build artifacts in ${countOf(cleanedCount, 'worktree')}`,
  ].filter(Boolean).join(' and ');
  return `Freed ${formatBytes(freedBytes)}${done ? `: ${done}` : ''}`;
}
