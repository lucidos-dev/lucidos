/**
 * What activating a workspace row does, and where it lands.
 *
 * One rule, three client shapes. Every row has a DEFAULT mode, which a plain
 * activation takes, and an ALTERNATE mode, which a right-click offers:
 *
 * | Client           | Default          | Alternate           |
 * |------------------|------------------|---------------------|
 * | Packaged desktop | separate window  | switch this window  |
 * | Browser tab      | switch in place  | new tab             |
 * | Installed PWA    | switch in place  | none                |
 *
 * The desktop default is a window because the client already thinks in windows.
 * ADR 0123 keys one per workspace and reopens the set on the next launch.
 * Repointing the window you are in is the one path fighting that. A browser
 * keeps left-click as a switch, since navigating in place is the web's
 * contract. All three workspace lists come through here, so the modes, the
 * label and the URL shape cannot drift between them: the gateway picker, the
 * Lucidos menu's switcher, and its notifications group.
 *
 * A row may also ask for a LANDING, a view inside the workspace it opens (see
 * `utils/workspaceLanding.ts`). It rides every mode, because a notification row
 * wants the same window rule as every other row plus the notifications view.
 */

import { openWorkspace } from '../api/client/control';
import { isMac, isStandalone, isTauri } from './platform';
import type { ClickModifiers } from './documentNavigation';
import { openNewTab } from './newTab';
import { showWorkspaceInNativeWindow } from './tauri';
import { landingHash, type WorkspaceLanding } from './workspaceLanding';
import type { WorkspaceState } from './workspaceState';

/** Where activating a row puts the workspace.
 *
 *  `separate` is a native window under Tauri and a browser tab elsewhere. They
 *  are one mode because they answer the same want, and because which of the two
 *  a client can give is not the caller's business. */
export type WorkspaceOpenMode = 'in-place' | 'separate';

/** The middle mouse button, the pointer shorthand for "open this beside". */
const MIDDLE_BUTTON = 1;

/** Can this client show a workspace anywhere but the current view?
 *
 *  False on an installed PWA, iOS and macOS alike. There `window.open` is
 *  unreliable in the worst direction. Safari hands the URL to the browser, so
 *  the "separate view" is the user ejected out of their app. The app popout
 *  hides itself for the same reason.
 *
 *  Tauri is answered first and unconditionally. The shell always has windows,
 *  and a webview's `display-mode` is not a promise worth depending on there. */
function separateViewIsAvailable(): boolean {
  return isTauri() || !isStandalone();
}

/** What a plain activation of a row does on this client. */
export function defaultOpenMode(): WorkspaceOpenMode {
  return isTauri() ? 'separate' : 'in-place';
}

/** The other mode, which a right-click offers, or `null` when a row has none.
 *  `currentId` is the workspace this view is serving, `null` on the picker.
 *
 *  An `unhealthy` workspace offers nothing at all. Opening one lands in a dead
 *  app shell, which `openOrRetry` and the switcher's unhealthy row already
 *  refuse. Past that the two alternates fail for different reasons.
 *
 *  A `separate` alternate needs somewhere to put it, so an installed PWA has
 *  none. A browser's CURRENT row keeps this one: a second tab on the workspace
 *  you are in is a real want.
 *
 *  An `in-place` alternate only says something when this view is already on
 *  some OTHER workspace. On the workspace you are in it is a no-op. On the
 *  picker the default already repoints this window, so offering it would put
 *  two affordances on one outcome. */
export function alternateOpenMode(
  state: WorkspaceState,
  currentId: string | null,
  rowId: string,
): WorkspaceOpenMode | null {
  if (state === 'unhealthy') return null;
  if (defaultOpenMode() === 'in-place') {
    return separateViewIsAvailable() ? 'separate' : null;
  }
  return currentId !== null && currentId !== rowId ? 'in-place' : null;
}

/** What to call a mode, in the row that offers it. This string is the menu row,
 *  the accessible name and the tooltip, so one that lies lies three times.
 *
 *  The desktop client has no tabs, so a label promising one would be wrong
 *  there. `in-place` is only ever an alternate under Tauri, which is why it may
 *  say "window" without qualifying it. */
export function openModeLabel(mode: WorkspaceOpenMode): string {
  if (mode === 'in-place') return 'Switch this window';
  return isTauri() ? 'Open in new window' : 'Open in new tab';
}

/** Does this gesture ask for a separate view? The web's own reading of it: the
 *  platform accelerator to open beside, and the middle button as the pointer
 *  shorthand for the same.
 *
 *  Cmd on a Mac and Ctrl elsewhere, deliberately not both. Shift and Alt are
 *  left plain: on a link they mean a new window and a download, and a row can
 *  offer neither. */
function asksForSeparateView(e: ClickModifiers): boolean {
  if (e.button === MIDDLE_BUTTON) return true;
  return (isMac ? e.metaKey : e.ctrlKey) === true;
}

/** Is this the macOS gesture for the CONTEXT MENU rather than a click?
 *
 *  Ctrl-click is a right-click there. Some engines dispatch a `click` beside the
 *  `contextmenu`, and `preventDefault` on the latter does not stop the former.
 *  A row that took it would unfold its action row and then navigate away from
 *  it. `isPlainClick` treats Ctrl the same way, for the same reason. */
function isMacContextClick(e: ClickModifiers): boolean {
  return isMac && e.ctrlKey === true && e.button !== MIDDLE_BUTTON;
}

/** The mode a click asks for, or `null` when the gesture is not an activation
 *  at all and the row must do nothing.
 *
 *  The gesture wins when this client can give it, and the default takes over
 *  when it cannot: a client with nowhere to put a separate view still owes a
 *  cmd-click something. Under Tauri no modifier gesture arrives (WKWebView drops
 *  them) and the default is already `separate`, so both arms agree there. */
export function openModeForClick(e: ClickModifiers): WorkspaceOpenMode | null {
  if (isMacContextClick(e)) return null;
  if (asksForSeparateView(e) && separateViewIsAvailable()) return 'separate';
  return defaultOpenMode();
}

/** May a MIDDLE click activate a row in `state`? Only where it would open a
 *  separate view.
 *
 *  A middle press means "open this beside". On a row with nowhere to open, it
 *  must do nothing rather than fall back to the row's primary action. On an
 *  `unhealthy` row that primary action is a RESTART. Answering a wheel press by
 *  rebooting an engine is the concrete bug this rule prevents. */
export function middleClickActivates(state: WorkspaceState): boolean {
  return separateViewIsAvailable() && state !== 'unhealthy';
}

/** An `onAuxClick` that runs `activate` for the middle button and nothing else.
 *
 *  A row needs this beside its `onClick` because a middle press dispatches NO
 *  `click`. Without it the row answers cmd-click and ignores the wheel, which
 *  are one intent to the user. `onAuxClick` also fires for the right button,
 *  whose meaning here is the action row, so that one is left alone. */
export function middleClickHandler(activate: (e: MouseEvent) => void) {
  return (e: MouseEvent): void => {
    if (e.button !== MIDDLE_BUTTON) return;
    e.preventDefault();
    activate(e);
  };
}

/** The path a workspace is served at, relative to whatever origin this client is
 *  on (ADR 0014), plus the fragment `landing` is delivered as. The same shape
 *  `openWorkspace` navigates to, so a new tab and a switch cannot disagree
 *  about where a workspace lives. */
export function workspacePath(id: string, landing?: WorkspaceLanding): string {
  return `/${encodeURIComponent(id)}/${landingHash(landing)}`;
}

/** The browser tab a workspace gets. Naming it is what makes a second
 *  activation land in the tab already open rather than stack a duplicate.
 *
 *  Keyed on the gateway slug, which is what the URL carries.
 *  `openThreadInWorkspace` names the same tab off the same key, so a thread link
 *  and a workspace row reach one tab between them. */
export function workspaceTabName(slug: string): string {
  return `lucidos-ws-${slug}`;
}

/** Put `id` where `mode` says, on the view `landing` names.
 *
 *  Only `separate` has two mechanisms. `in-place` is a replacing navigation on
 *  every client, including the desktop one, where it is the alternate a
 *  right-click offers.
 *
 *  Under Tauri the SHELL decides which window a `separate` activation lands in.
 *  It focuses one already on the workspace, repoints a calling window that is on
 *  the picker, or opens a new one. Only the shell can see every window, so only
 *  the shell can choose. It takes the landing by NAME and composes the fragment
 *  itself, for the reason `utils/workspaceLanding.ts` gives.
 *
 *  Rejects rather than reporting: the call sites surface a failure differently,
 *  the picker on its own error line and the menu as a toast. Every caller is a
 *  direct click, so a swallowed rejection would be a button that did nothing. */
export async function openWorkspaceIn(
  mode: WorkspaceOpenMode,
  id: string,
  landing?: WorkspaceLanding,
): Promise<void> {
  if (mode === 'in-place') {
    openWorkspace(id, landing);
    return;
  }
  if (isTauri()) {
    await showWorkspaceInNativeWindow(id, landing);
    return;
  }
  if (!openNewTab(workspacePath(id, landing), workspaceTabName(id))) {
    throw new Error('Your browser blocked the new tab. Allow pop-ups for this site.');
  }
}
