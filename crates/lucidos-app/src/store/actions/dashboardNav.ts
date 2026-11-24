
import { setDashboardPanelOpen } from './thread-navigation';
import { navigateToPane, showDashboardRail } from './pane';
import { mobileView } from '../store';
import { viewportIsMobile } from '../../utils/viewport';

export function revealDashboard(): void {
  if (!viewportIsMobile.value) {
    showDashboardRail();
    return;
  }
  if (mobileView.value !== 'thread') navigateToPane('thread');
  document.dispatchEvent(new Event('reveal-mobile-bars'));
  setDashboardPanelOpen(true);
}
