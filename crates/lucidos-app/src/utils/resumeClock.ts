/** When the page last came back to the foreground. The app frame load probe
 *  reads it, to tell a stall that follows a resume from one that does not. A
 *  temporary measure: see `docs/temporary-measures.md` § App frame load probe. */

import { onPageWake } from './pageVisit';

let lastResumeAt: number | null = null;

export function sinceResumeMs(now = Date.now()): number | null {
  return lastResumeAt === null ? null : now - lastResumeAt;
}

export function installResumeClock(): () => void {
  return onPageWake(() => {
    lastResumeAt = Date.now();
  });
}

export function _resetResumeClockForTesting(): void {
  lastResumeAt = null;
}
