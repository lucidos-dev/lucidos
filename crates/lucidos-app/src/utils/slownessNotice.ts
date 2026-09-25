import type { MemoryUser, ProcessorUser } from '../api/client/control';

/** Decimal gigabytes, as the gateway's log line prints them. */
export function formatGigabytes(bytes: number): string {
  return `${(bytes / 1e9).toFixed(1)} GB`;
}

/** "Google Chrome 7.0 GB, Slack 0.6 GB, Lucidos 1.4 GB". */
export function memoryUsersPhrase(users: MemoryUser[]): string {
  return users.map(u => `${u.name} ${formatGigabytes(u.bytes)}`).join(', ');
}

/** One thing to do, about the biggest user.
 *
 *  Only an app is named: a bare process may be a system service, and "quit
 *  com.apple.Virtualization.VirtualMachine" is not advice. When Lucidos itself
 *  is biggest, the lever is its coding agents, never quitting it. */
export function memoryRecommendation(users: MemoryUser[]): string {
  const top = users[0];
  if (top?.kind === 'lucidos') return 'Stop coding-agent threads you are not using.';
  if (top?.kind === 'app') return `Quit or restart ${top.name} to free memory.`;
  return 'Quit apps you are not using to free memory.';
}

/** The share of the whole computer at which the busiest app is worth naming.
 *  Below it, nothing stands out, and a list would only point at bystanders. */
export const STANDS_OUT_PERCENT = 10;

/** The busiest apps worth naming: all of them once the top one stands out,
 *  none otherwise. */
export function busyEnoughToName(users: ProcessorUser[]): ProcessorUser[] {
  return (users[0]?.percent ?? 0) >= STANDS_OUT_PERCENT ? users : [];
}

/** "Xcode 40%, Lucidos 12%". */
export function processorUsersPhrase(users: ProcessorUser[]): string {
  return users.map(u => `${u.name} ${u.percent}%`).join(', ');
}

/** One thing to do when Lucidos is slow and memory is not the clear cause.
 *  Same rules as {@link memoryRecommendation}, plus an honest fallback when
 *  nothing is busy: the cause is then out of reach of this measurement. */
export function processorRecommendation(users: ProcessorUser[]): string {
  const top = busyEnoughToName(users)[0];
  if (!top) return 'Nothing on this computer stands out as busy. If it lasts, restart the computer.';
  if (top.kind === 'lucidos') return 'Stop coding-agent threads you are not using.';
  if (top.kind === 'app') return `Quit or restart ${top.name}.`;
  return 'Quit apps you are not using.';
}
