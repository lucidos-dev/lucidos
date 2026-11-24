import type { DatabaseProblem, MemoryUser, ProcessorUser } from '../api/client/control';

export function formatGigabytes(bytes: number): string {
  return `${(bytes / 1e9).toFixed(1)} GB`;
}

export function memoryUsersPhrase(users: MemoryUser[]): string {
  return users.map(u => `${u.name} ${formatGigabytes(u.bytes)}`).join(', ');
}

export function memoryRecommendation(users: MemoryUser[]): string {
  const top = users[0];
  if (top?.kind === 'lucidos') return 'Stop coding-agent threads you are not using.';
  if (top?.kind === 'app') return `Quit or restart ${top.name} to free memory.`;
  return 'Quit apps you are not using to free memory.';
}

export const STANDS_OUT_PERCENT = 10;

export function busyEnoughToName(users: ProcessorUser[]): ProcessorUser[] {
  return (users[0]?.percent ?? 0) >= STANDS_OUT_PERCENT ? users : [];
}

export function processorUsersPhrase(users: ProcessorUser[]): string {
  return users.map(u => `${u.name} ${u.percent}%`).join(', ');
}

export function processorRecommendation(users: ProcessorUser[]): string {
  const top = busyEnoughToName(users)[0];
  if (!top) {
    return 'Lucidos found no database or disk problem, and nothing on this computer stands out as busy. '
      + 'If it lasts, restart the computer.';
  }
  if (top.kind === 'lucidos') return 'Stop coding-agent threads you are not using.';
  if (top.kind === 'app') return `Quit or restart ${top.name}.`;
  return 'Quit apps you are not using.';
}

function databaseRunner(packaged: boolean): string {
  return packaged ? 'Lucidos' : 'Docker';
}

export function databaseRecommendation(problem: DatabaseProblem, packaged: boolean): string {
  if (problem === 'pool_exhausted') {
    return 'All its database connections are busy. Stop coding-agent threads you are not using. '
      + 'If it lasts, restart Lucidos.';
  }
  return `Restart ${databaseRunner(packaged)}.`;
}

export function diskRecommendation(freeBytes: number, packaged: boolean): string {
  return `${formatGigabytes(freeBytes)} free. Free up disk space. `
    + `If Lucidos stays slow after that, restart ${databaseRunner(packaged)}.`;
}
