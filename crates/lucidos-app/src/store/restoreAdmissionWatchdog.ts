/** Watches restore admission for a stall, so a page stuck behind it says where
 *  and retries the stream. A stall never opens admission: only a completed
 *  hello does (`restoreRuntime.ts`). */
import { postClientLog } from '../utils/clientLog';
import { restoreAdmissionPhase } from '../utils/restoreAdmission';
import { DEVICE_DATABASE_STALL_MS } from './deviceDatabase';

/** How long one connection may wait for admission before it counts as
 *  stalled. Longer than a first device database bound, so a hung storage step
 *  fails and reports itself before the watchdog speaks. */
export const RESTORE_ADMISSION_STALL_MS = 2 * DEVICE_DATABASE_STALL_MS;

let waitingSince: number | null = null;
let stallPosted = false;
let timer: ReturnType<typeof setTimeout> | null = null;

/** Arm for a new connection epoch. The wait counts from the first epoch that
 *  was not admitted, so a retry chain reports its whole wait. */
export function watchRestoreAdmission(restart: () => void): void {
  stopWatchingRestoreAdmission();
  waitingSince ??= Date.now();
  timer = setTimeout(() => {
    timer = null;
    const phase = restoreAdmissionPhase();
    if (phase === 'admitted') return;
    if (!stallPosted) {
      stallPosted = true;
      postClientLog('restore-admission', 'stalled', { phase, waited_ms: waitedMs() });
    }
    // A hello that never came may sit behind a half-open upstream. A stall
    // after it is local storage, which a new stream cannot reach.
    if (phase === 'awaiting-hello') restart();
  }, RESTORE_ADMISSION_STALL_MS);
}

/** The stream is gone, so nothing is left to restart. The wait keeps counting
 *  until a later connection is admitted. */
export function stopWatchingRestoreAdmission(): void {
  if (timer) clearTimeout(timer);
  timer = null;
}

export function noteRestoreAdmitted(): void {
  stopWatchingRestoreAdmission();
  if (stallPosted) postClientLog('restore-admission', 'admitted_after_stall', { waited_ms: waitedMs() });
  waitingSince = null;
  stallPosted = false;
}

function waitedMs(): number {
  return waitingSince === null ? 0 : Date.now() - waitingSince;
}
