/** The restore guard (`docs/glossary.md` § restore guard). A launch never
 *  restores a thread the previous session died showing. So a thread whose
 *  render kills the page cannot lock the device out of the app.
 *
 *  Focusing a thread arms a device-local marker naming it. The marker clears
 *  once the page has stayed visible for the survival window, on `pagehide`, or
 *  on a hidden visibility change. A launch that finds the marker naming the
 *  stored focus withholds that focus. Device-local only: it never writes
 *  server state.
 *
 *  Imports nothing from `store.ts`, whose focus signal initializer calls
 *  {@link readGuardedFocus}, so the import back would be a boot-time cycle. */

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

let withheldThreadId: string | null = null;
let survivalTimer: ReturnType<typeof setTimeout> | null = null;

/** The stored focus, read through the guard. A withheld focus is dropped from
 *  storage, so the rest of the boot sees a device with no stored focus and
 *  lands on Home. Called once, by the focus signal's initializer. */
export function readGuardedFocus(focusKey: string): string | null {
  const { focus, withheld } = restoredFocus(
    localStorage.getItem(focusKey),
    localStorage.getItem(RESTORE_GUARD_KEY),
  );
  if (withheld !== null) {
    localStorage.removeItem(focusKey);
    localStorage.removeItem(RESTORE_GUARD_KEY);
    withheldThreadId = withheld;
  }
  return focus;
}

/** The thread this launch withheld, once. Later calls return `null`. */
export function takeWithheldThread(): string | null {
  const id = withheldThreadId;
  withheldThreadId = null;
  return id;
}

/** Name the focused thread in the marker and start the survival window. A
 *  `null` focus has nothing to guard, so it clears the marker instead. */
export function armRestoreGuard(threadId: string | null): void {
  if (threadId === null) {
    disarmRestoreGuard();
    return;
  }
  cancelSurvival();
  localStorage.setItem(RESTORE_GUARD_KEY, threadId);
  survivalTimer = setTimeout(disarmRestoreGuard, RESTORE_GUARD_SURVIVAL_MS);
}

export function disarmRestoreGuard(): void {
  cancelSurvival();
  localStorage.removeItem(RESTORE_GUARD_KEY);
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
