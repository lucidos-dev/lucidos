import { signal } from '@preact/signals';
import { pushOverlay, removeOverlay } from './overlayStack';

/** Escape registry id. The panel is NOT an `<Overlay>` (it is a view inside a
 *  pane, not something floating over other UI, so it neither dismisses on an
 *  outside click nor makes anything inert), but Escape should still close it,
 *  and Escape has exactly one owner app-wide: the LIFO `overlayStack` the
 *  central dispatcher pops. Same panel-less registration `ContentHeaderActions`
 *  uses for pseudo-fullscreen. */
const OVERLAY_ID = 'thread-filter-panel';

/** Showing the filters is a state OF THE DRAWER, so it survives a reload the
 *  same way the drawer's other states do (`lucidos-alt-view`, the channel
 *  selection, the drawer's own open/closed, its width). Cleared rather than
 *  written `false` for the default, so a pristine state restores pristine
 *  (`setDrawerView`'s convention). */
const PANEL_OPEN_KEY = 'lucidos-thread-filter-panel-open';

/** Whether the thread filter panel is showing. The panel renders INSIDE the
 *  thread drawer pane (`ThreadDrawer`, which backs the desktop drawer and the
 *  mobile threads pane alike) while its toggle lives in the threads header, so
 *  the state cannot be local component state the way it was while the filter
 *  was an anchored dropdown: the two headers each instantiate
 *  `useThreadsHeaderState`, and neither of them is where the panel renders. */
export const threadFilterPanelOpen = signal(localStorage.getItem(PANEL_OPEN_KEY) === 'true');

/** All three parts of the state move together, and only here: the signal, the
 *  persisted key, and the Escape registration. Tying registration to the STATE
 *  rather than to a mount effect keeps the panel component hook-free at its own
 *  level (its unit test invokes it directly) and makes the stack entry
 *  impossible to leak past a close. */
function setPanelOpen(open: boolean): void {
  threadFilterPanelOpen.value = open;
  if (open) localStorage.setItem(PANEL_OPEN_KEY, 'true');
  else localStorage.removeItem(PANEL_OPEN_KEY);
  syncEscapeRegistration();
}

function syncEscapeRegistration(): void {
  if (threadFilterPanelOpen.value) pushOverlay({ id: OVERLAY_ID, dismiss: closeThreadFilterPanel });
  else removeOverlay(OVERLAY_ID);
}

// A panel RESTORED open has to take its Escape entry here, at load, because
// nothing else will: the registration rides `openThreadFilterPanel`, and a
// restore never calls it. Missing it is not a stale-stack-entry bug (the
// registration is keyed on the id, so it cannot double up) but the opposite:
// the panel would be up on screen and deaf to Escape until the user toggled it
// off and on again. No-op on the ordinary closed boot.
syncEscapeRegistration();

export function openThreadFilterPanel(): void {
  setPanelOpen(true);
}

export function closeThreadFilterPanel(): void {
  setPanelOpen(false);
}

export function toggleThreadFilterPanel(): void {
  setPanelOpen(!threadFilterPanelOpen.value);
}
