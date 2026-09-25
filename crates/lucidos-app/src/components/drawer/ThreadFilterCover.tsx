import { threadFilterPanelOpen, closeThreadFilterPanel } from '../../store/threadFilterPanel';
import { scaledDurationMs } from '../../utils/motion';
import { useLingeringFlag } from '../../hooks/useDelayedLoading';
import { ThreadFilterPanel } from '../layout/ThreadFilterPanel';

/** The panel's fade, the 1x `--duration-fast` (global/base.css). The slack is
 *  a fixed margin past it, so it stays outside the scaled term. */
const FILTER_PANEL_FADE_MS = 150;
const FILTER_PANEL_FADE_SLACK_MS = 50;

/** The box the thread filter panel shows in, over the drawer's list.
 *
 *  A fade THROUGH, never a crossfade: the panel wears the list's own geometry,
 *  so both drawn at once print rows and options on the same lines. The opaque
 *  cover shows at once, and hides only when the fade layer inside it has
 *  finished (drawer.css).
 *
 *  The cover and the fade layer are always mounted, so each fade runs on an
 *  element that already exists: it reverses mid-way, and it starts in the
 *  same frame as the header's. The panel mounts on open and stays for the
 *  fade out. Everything else (Escape, the overlay stack, the drawer's keys,
 *  the pressed Filter button) follows the open SIGNAL at once. A leaving cover
 *  is `inert`, so it takes no pointer and no focus. */
export function ThreadFilterCover() {
  const open = threadFilterPanelOpen.value;
  const rendered = useLingeringFlag(open, scaledDurationMs(FILTER_PANEL_FADE_MS) + FILTER_PANEL_FADE_SLACK_MS);
  return (
    <div class="thread-filter-cover" data-open={open ? '' : undefined} inert={!open}>
      <div class="thread-filter-fade">
        {rendered && <ThreadFilterPanel onClose={closeThreadFilterPanel} />}
      </div>
    </div>
  );
}
