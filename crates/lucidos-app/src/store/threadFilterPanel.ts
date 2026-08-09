import { signal } from '@preact/signals';
import { pushOverlay, removeOverlay } from './overlayStack';

/** Whether the thread filter panel is showing. The panel renders INSIDE the
 *  thread drawer pane (`ThreadDrawer`, which backs the desktop drawer and the
 *  mobile threads pane alike) while its toggle lives in the threads header, so
 *  the state cannot be local component state the way it was while the filter
 *  was an anchored dropdown: the two headers each instantiate
 *  `useThreadsHeaderState`, and neither of them is where the panel renders. */
export const threadFilterPanelOpen = signal(false);

/** Escape registry id. The panel is NOT an `<Overlay>` (it is a view inside a
 *  pane, not something floating over other UI, so it neither dismisses on an
 *  outside click nor makes anything inert), but Escape should still close it,
 *  and Escape has exactly one owner app-wide: the LIFO `overlayStack` the
 *  central dispatcher pops. Same panel-less registration `ContentHeaderActions`
 *  uses for pseudo-fullscreen. */
const OVERLAY_ID = 'thread-filter-panel';

/** Registration is tied to the STATE, not to a mount effect: it keeps the panel
 *  component hook-free at its own level (its unit test invokes it directly) and
 *  makes the stack entry impossible to leak past a close. */
export function openThreadFilterPanel(): void {
  threadFilterPanelOpen.value = true;
  pushOverlay({ id: OVERLAY_ID, dismiss: closeThreadFilterPanel });
}

export function closeThreadFilterPanel(): void {
  threadFilterPanelOpen.value = false;
  removeOverlay(OVERLAY_ID);
}

export function toggleThreadFilterPanel(): void {
  if (threadFilterPanelOpen.value) closeThreadFilterPanel();
  else openThreadFilterPanel();
}
