/** Revealing the dashboard (ADR 0432). Kept apart from the dashboard reads,
 *  so the SSE path never pulls the menu's loaders into the entry chunk. */

import { openDashboardSheet } from '../dashboards';
import { navigateToPane, showDashboardRail } from './pane';
import { mobileView } from '../store';
import { viewportIsMobile } from '../../utils/viewport';

/** The dashboard wherever this layout keeps it: the rail on desktop, shown if it
 *  was hidden, and the dashboard sheet over the open thread on a phone. */
export function revealDashboard(): void {
  if (!viewportIsMobile.value) {
    showDashboardRail();
    return;
  }
  // The sheet belongs to the open thread and hangs below the header holding
  // its toggle. So land on the thread, and bring a scrolled-away header back.
  if (mobileView.value !== 'thread') navigateToPane('thread');
  document.dispatchEvent(new Event('reveal-mobile-bars'));
  openDashboardSheet();
}
