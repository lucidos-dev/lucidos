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
  // The sheet hangs from the thread pane's title row, so land there first.
  if (mobileView.value !== 'thread') navigateToPane('thread');
  openDashboardSheet();
}
