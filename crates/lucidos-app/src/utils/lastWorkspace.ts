/**
 * "Last active workspace" memory (gateway topology, ADR 0014).
 *
 * The gateway shows the picker at the smart root (`/`, when several workspaces
 * are registered) and at the sigil (`/~/`, where the installed PWA launches via
 * its manifest `start_url`). To "go to where the user was last", we remember the
 * workspace the user was in and auto-open it whenever the picker loads (unless
 * the URL carries `?pick`, the explicit "show me the list" escape).
 *
 * The id is stored in `localStorage` under a DEVICE-GLOBAL key (see
 * `workspaceStorage.ts` § GLOBAL_KEYS): it is written from inside a workspace
 * (`/<slug>/`, where storage is namespaced `ws:<slug>:…`) but read by the picker
 * (`/~/` or `/`, where namespacing is a no-op and storage is raw). A namespaced
 * key would never match across those contexts — so it MUST stay raw on both ends.
 *
 * Per-device by design: each browser/PWA remembers its own last workspace.
 */

/** Device-global localStorage key holding the last-opened workspace slug. */
export const LAST_WORKSPACE_KEY = 'lucidos-last-workspace';

/** Record `id` as the last-active workspace. Best-effort: a disabled/hostile
 *  `localStorage` just means the picker won't auto-open next time. */
export function rememberLastWorkspace(id: string): void {
  try {
    localStorage.setItem(LAST_WORKSPACE_KEY, id);
  } catch {
    /* storage unavailable — degrade to showing the picker on reopen */
  }
}

/** The last-active workspace slug, or `null` if none recorded / storage off. */
export function recallLastWorkspace(): string | null {
  try {
    return localStorage.getItem(LAST_WORKSPACE_KEY);
  } catch {
    return null;
  }
}

/** Drop the remembered workspace (e.g. it was deleted, so don't keep retrying). */
export function forgetLastWorkspace(): void {
  try {
    localStorage.removeItem(LAST_WORKSPACE_KEY);
  } catch {
    /* no-op */
  }
}
