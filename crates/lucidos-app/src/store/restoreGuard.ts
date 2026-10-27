/** The restore guard (`docs/glossary.md` § restore guard). A launch never
 *  restores a thread the previous session died showing. So a thread whose
 *  render kills the page cannot lock the device out of the app.
 *
 *  Focusing a thread arms a window-scoped marker naming it and this window.
 *  The marker clears once the page has stayed visible for the survival window,
 *  on `pagehide`, or on a hidden visibility change. A launch that finds the
 *  marker naming the stored focus withholds that focus. When another window
 *  armed the marker and is still alive, the boot releases it
 *  (`startup.ts`). Device-local only: it never writes server state.
 *
 *  Imports nothing from `store.ts`, whose focus signal initializer calls
 *  {@link readGuardedFocus}, so the import back would be a boot-time cycle. */

import {
  clearOwnWindowItem, readSeedItem, readWindowItem, removeWindowItem, windowId, writeWindowItem,
} from '../utils/windowStorage';

export const RESTORE_GUARD_KEY = 'lucidos-restore-guard';
export const RESTORE_GUARD_SURVIVAL_MS = 20_000;

/** What a launch restores, given the stored focus and the guard's marker. */
export function restoredFocus(
  storedFocus: string | null,
  marker: string | null,
): { focus: string | null; withheld: string | null } {
  if (storedFocus !== null && marker === storedFocus) {
    return { focus: null, withheld: storedFocus };
  }
  return { focus: storedFocus, withheld: null };
}

/** A withheld thread, and the window whose marker named it. */
export interface WithheldThread {
  threadId: string;
  armedBy: string;
}

let withheldThread: WithheldThread | null = null;
let survivalTimer: ReturnType<typeof setTimeout> | null = null;

/** The marker names the thread and the window that armed it. A window with no
 *  marker of its own reads the shared seed, which another window may have
 *  armed while it is still alive. */
function readMarker(): { threadId: string; armedBy: string } | null {
  const raw = readWindowItem(RESTORE_GUARD_KEY);
  if (raw === null) return null;
  const [threadId, armedBy = ''] = raw.split(' ');
  return { threadId, armedBy };
}

/** The stored focus, read through the guard. A withheld focus is dropped from
 *  storage, so the rest of the boot sees a window with no stored focus and
 *  lands on Home. Called once, by the focus signal's initializer. */
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

/** The thread this launch withheld, once. Later calls return `null`. */
export function takeWithheldThread(): WithheldThread | null {
  const withheld = withheldThread;
  withheldThread = null;
  return withheld;
}

/** Name the focused thread in the marker and start the survival window. A
 *  `null` focus has nothing to guard, so it clears the marker instead. */
export function armRestoreGuard(threadId: string | null): void {
  if (threadId === null) {
    disarmRestoreGuard();
    return;
  }
  cancelSurvival();
  writeWindowItem(RESTORE_GUARD_KEY, `${threadId} ${windowId()}`);
  survivalTimer = setTimeout(disarmRestoreGuard, RESTORE_GUARD_SURVIVAL_MS);
}

/** Clear this window's marker, and the shared seed only when this window
 *  armed it: a hidden window must not disarm another window's guard. */
export function disarmRestoreGuard(): void {
  cancelSurvival();
  if (readSeedItem(RESTORE_GUARD_KEY)?.endsWith(` ${windowId()}`)) removeWindowItem(RESTORE_GUARD_KEY);
  else clearOwnWindowItem(RESTORE_GUARD_KEY);
}

function cancelSurvival(): void {
  if (survivalTimer !== null) clearTimeout(survivalTimer);
  survivalTimer = null;
}

/** Clear the marker whenever the page exits cleanly, and arm it again for the
 *  current focus whenever the page comes back. Returns the teardown. */
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
