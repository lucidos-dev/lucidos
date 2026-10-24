/** When the page last came back to the foreground. The app frame load probe
 *  reads it, to tell a stall that follows a resume from one that does not. A
 *  temporary measure: see `docs/temporary-measures.md` § App frame load probe. */

import { onPageWake } from './pageVisit';

let lastResumeAt: number | null = null;

/** Milliseconds since the page last came back to the foreground, or `null` when
 *  it has not been away since the clock started. */
export function sinceResumeMs(now = Date.now()): number | null {
  return lastResumeAt === null ? null : now - lastResumeAt;
}

/** Start the clock on every kind of wake `onPageWake` reports. Returns a
 *  teardown for a hot update. */
export function installResumeClock(): () => void {
  return onPageWake(() => {
    lastResumeAt = Date.now();
  });
}

/** Test-only: forget the last resume. */
export function _resetResumeClockForTesting(): void {
  lastResumeAt = null;
}
