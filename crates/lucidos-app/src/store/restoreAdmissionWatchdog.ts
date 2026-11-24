import { postClientLog } from '../utils/clientLog';
import { restoreAdmissionPhase } from '../utils/restoreAdmission';
import { DEVICE_DATABASE_STALL_MS } from './deviceDatabase';

export const RESTORE_ADMISSION_STALL_MS = 2 * DEVICE_DATABASE_STALL_MS;

let waitingSince: number | null = null;
let stallPosted = false;
let timer: ReturnType<typeof setTimeout> | null = null;

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
    if (phase === 'awaiting-hello') restart();
  }, RESTORE_ADMISSION_STALL_MS);
}

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
