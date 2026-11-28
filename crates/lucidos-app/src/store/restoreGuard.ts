
import {
  clearOwnWindowItem, readSeedItem, readWindowItem, removeWindowItem, windowId, writeWindowItem,
} from '../utils/windowStorage';

export const RESTORE_GUARD_KEY = 'lucidos-restore-guard';
export const RESTORE_GUARD_SURVIVAL_MS = 20_000;

export function restoredFocus(
  storedFocus: string | null,
  marker: string | null,
): { focus: string | null; withheld: string | null } {
  if (storedFocus !== null && marker === storedFocus) {
    return { focus: null, withheld: storedFocus };
  }
  return { focus: storedFocus, withheld: null };
}

export interface WithheldThread {
  threadId: string;
  armedBy: string;
}

let withheldThread: WithheldThread | null = null;
let survivalTimer: ReturnType<typeof setTimeout> | null = null;

function readMarker(): { threadId: string; armedBy: string } | null {
  const raw = readWindowItem(RESTORE_GUARD_KEY);
  if (raw === null) return null;
  const [threadId, armedBy = ''] = raw.split(' ');
  return { threadId, armedBy };
}

export function readGuardedFocus(focusKey: string): string | null {
  const marker = readMarker();
  const { focus, withheld } = restoredFocus(readWindowItem(focusKey), marker?.threadId ?? null);
  if (withheld !== null) {
    removeWindowItem(focusKey);
    clearOwnWindowItem(RESTORE_GUARD_KEY);
    withheldThread = { threadId: withheld, armedBy: marker?.armedBy ?? '' };
  }
  return focus;
}

export function takeWithheldThread(): WithheldThread | null {
  const withheld = withheldThread;
  withheldThread = null;
  return withheld;
}

export function armRestoreGuard(threadId: string | null): void {
  if (threadId === null || document.visibilityState === 'hidden') {
    disarmRestoreGuard();
    return;
  }
  cancelSurvival();
  writeWindowItem(RESTORE_GUARD_KEY, `${threadId} ${windowId()}`);
  survivalTimer = setTimeout(disarmRestoreGuard, RESTORE_GUARD_SURVIVAL_MS);
}

export function disarmRestoreGuard(): void {
  cancelSurvival();
  if (readSeedItem(RESTORE_GUARD_KEY)?.endsWith(` ${windowId()}`)) removeWindowItem(RESTORE_GUARD_KEY);
  else clearOwnWindowItem(RESTORE_GUARD_KEY);
}

function cancelSurvival(): void {
  if (survivalTimer !== null) clearTimeout(survivalTimer);
  survivalTimer = null;
}

export function installRestoreGuard(currentFocus: () => string | null): () => void {
  const onVisibilityChange = () => {
    if (document.visibilityState === 'hidden') disarmRestoreGuard();
    else armRestoreGuard(currentFocus());
  };
  document.addEventListener('visibilitychange', onVisibilityChange);
  window.addEventListener('pagehide', disarmRestoreGuard);
  if (document.visibilityState !== 'hidden') armRestoreGuard(currentFocus());
  return () => {
    document.removeEventListener('visibilitychange', onVisibilityChange);
    window.removeEventListener('pagehide', disarmRestoreGuard);
    cancelSurvival();
  };
}
