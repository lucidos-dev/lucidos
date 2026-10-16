/** Landing on the dashboard (ADR 0432). Kept apart from the dashboard reads,
 *  so the SSE path never pulls the menu's loaders into the entry chunk. */

import { landNextHomeOnDashboard } from '../dashboards';
import { showDashboardRail } from './pane';
import { openHomeThread } from './homeThread';
import { viewportIsMobile } from '../../utils/viewport';

/** A tap on the Home icon, or the Lucidos menu's Home row. On desktop it opens
 *  the Home chat only, even in Home: the rail's own toggle shows the dashboard.
 *  On a phone it opens Home on its dashboard face, sliding the chat down if it
 *  was up. */
export function openHome(): void {
  if (!viewportIsMobile.value) {
    openHomeThread();
    return;
  }
  landNextHomeOnDashboard();
  openHomeThread();
  // The keyboard belongs to the chat face.
  (document.activeElement as HTMLElement | null)?.blur?.();
}

/** The dashboard wherever this layout keeps it: the rail on desktop, shown if it
 *  was hidden, and Home's dashboard face on a phone. */
export function revealDashboard(): void {
  if (viewportIsMobile.value) openHome();
  else showDashboardRail();
}
