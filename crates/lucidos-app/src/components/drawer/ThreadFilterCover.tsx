import type { Signal } from '@preact/signals';
import { threadFilterPanelOpen, closeThreadFilterPanel } from '../../store/threadFilterPanel';
import { drawerView, type DrawerView } from '../../store/store';
import { ThreadFilterPanel } from '../layout/ThreadFilterPanel';
import { NavigationCover } from '../shared/NavigationCover';
import { LeavingViewDrawing, type LeavingDrawing } from './LeavingViewDrawing';

/** What the drawer shows, as its navigation cover key: the filter panel, or
 *  the list under one status filter. Every change to it is one dip, so a status
 *  change plays the same transition whoever made it. A panel row changes both
 *  halves in one tick, which is still one change. */
export function drawerSwapKey(filtersOpen: boolean, view: DrawerView): string {
  return filtersOpen ? 'filters' : `threads:${view}`;
}

/** The layers over the drawer's list, and the navigation cover that hides
 *  every swap between what they show.
 *
 *  The swap dips through the pane background, the same every time: the cover
 *  rises over the leaving view, holds, and clears off the arriving one. The
 *  leaving view stays on screen until the dip's midpoint (drawer.css). That is
 *  the filter panel when it closes, and a drawing of the old list when only the
 *  status changes (`LeavingViewDrawing`). So the two views never show together.
 *
 *  The filter cover and the panel inside it are always mounted, and hidden
 *  while shut, so opening costs no render. Everything else (Escape, the overlay
 *  stack, the drawer's keys, the pressed Filter button) follows the open
 *  SIGNAL. A shut cover is `inert`, so it takes no pointer and no focus. So is
 *  an open one on a collapsed drawer, since the panel stays open there. */
export function ThreadFilterCover({ paneVisible, leaving }: {
  paneVisible: boolean;
  leaving: Signal<LeavingDrawing | null>;
}) {
  const open = threadFilterPanelOpen.value;
  const swapKey = drawerSwapKey(open, drawerView.value);
  return (
    <>
      <LeavingViewDrawing swapKey={swapKey} drawing={leaving} />
      <div class="thread-filter-cover" data-open={open ? '' : undefined} inert={!open || !paneVisible}>
        <ThreadFilterPanel onClose={closeThreadFilterPanel} />
      </div>
      <NavigationCover viewKey={swapKey} motion="dip" />
    </>
  );
}
