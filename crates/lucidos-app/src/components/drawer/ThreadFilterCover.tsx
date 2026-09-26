import { threadFilterPanelOpen, closeThreadFilterPanel } from '../../store/threadFilterPanel';
import { ThreadFilterPanel } from '../layout/ThreadFilterPanel';
import { NavigationCover } from '../shared/NavigationCover';

/** The drawer's two views, Threads and Filters, as a navigation cover key. The
 *  pane title (`ThreadsPaneTitle`) arrives on the same key. */
export function drawerViewKey(filtersOpen: boolean): 'filters' | 'threads' {
  return filtersOpen ? 'filters' : 'threads';
}

/** The box the thread filter panel shows in, over the drawer's list, and the
 *  navigation cover that hides the swap between them.
 *
 *  The swap moves exactly like a content-pane navigation: the leaving view goes
 *  at once, and the arriving one fades in from behind the same cover. The
 *  filter cover itself only shows or hides.
 *
 *  The cover and the panel inside it are always mounted, and hidden while
 *  shut, so opening costs no render. Everything else (Escape, the overlay
 *  stack, the drawer's keys, the pressed Filter button) follows the open
 *  SIGNAL. A shut cover is `inert`, so it takes no pointer and no focus. So is
 *  an open one on a collapsed drawer, since the panel stays open there. */
export function ThreadFilterCover({ paneVisible }: { paneVisible: boolean }) {
  const open = threadFilterPanelOpen.value;
  return (
    <>
      <div class="thread-filter-cover" data-open={open ? '' : undefined} inert={!open || !paneVisible}>
        <ThreadFilterPanel onClose={closeThreadFilterPanel} />
      </div>
      <NavigationCover viewKey={drawerViewKey(open)} />
    </>
  );
}
