/**
 * Open a workspace in a SECOND client window, from a workspace row.
 *
 * One concept, two mechanisms, and the split is the whole of this module. A
 * browser opens a tab. The packaged desktop client opens a native window through
 * `open_workspace_window`, because WKWebView silently drops `window.open`: wry
 * installs a new-window delegate only for a builder that asks for one, and no
 * app window does (see the desktop popout plan).
 *
 * Both surfaces that list workspaces come through here, so the label, the
 * platform gate and the URL shape cannot drift between them. Everything is a
 * plain function rather than a store action, because the picker is its own
 * render root with no global store.
 */

import { isTauri, isIOSPwa } from './platform';
import { openNewTab } from './newTab';
import { openWorkspaceInNativeWindow } from './tauri';
import type { WorkspaceState } from './workspaceState';

/** Can this client show a workspace anywhere but the current view?
 *
 *  False on an installed iOS PWA alone. There `window.open` hands the URL to
 *  Safari, so the "second window" is the user leaving the app. That is worse
 *  than the row they already have, and the app popout hides itself for the same
 *  reason. */
function canOpenWorkspaceWindow(): boolean {
  return !isIOSPwa();
}

/** Does a row in `state` offer this at all? One rule for both workspace lists,
 *  so neither can quietly disagree with the other about which rows carry it.
 *
 *  An `unhealthy` workspace does not. Opening one lands in a dead app shell,
 *  which `openOrRetry` and the switcher's unhealthy row already refuse. A
 *  second window would be that same shell in a new frame. `stopped` and
 *  `booting` do offer it, exactly as they are openable in place. */
export function offersWorkspaceWindow(state: WorkspaceState): boolean {
  return canOpenWorkspaceWindow() && state !== 'unhealthy';
}

/** What to call the action, in the row that offers it.
 *
 *  The desktop client has no tabs and the destination is a real window, so a
 *  label promising a tab would be wrong there. This string is the menu row, the
 *  accessible name and the tooltip, so one that lies lies three times. */
export function workspaceWindowLabel(): string {
  return isTauri() ? 'Open in new window' : 'Open in new tab';
}

/** The path a workspace is served at, relative to whatever origin this client is
 *  on (ADR 0014). The same shape `openWorkspace` navigates to, so a new tab and
 *  a switch cannot disagree about where a workspace lives. */
export function workspacePath(id: string): string {
  return `/${encodeURIComponent(id)}/`;
}

/** Open `id` in a new window (desktop) or a new tab (browser).
 *
 *  Rejects rather than reporting: the two call sites surface a failure
 *  differently, the picker on its own error line and the switcher as a toast.
 *  Every caller is a direct click, so a swallowed rejection would be a button
 *  that did nothing. */
export async function openWorkspaceWindow(id: string): Promise<void> {
  if (isTauri()) {
    await openWorkspaceInNativeWindow(id);
    return;
  }
  if (!openNewTab(workspacePath(id))) {
    throw new Error('Your browser blocked the new tab. Allow pop-ups for this site.');
  }
}
