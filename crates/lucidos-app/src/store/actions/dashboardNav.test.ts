/**
 * On desktop the dashboard lives in its rail. On a phone it is the dashboard
 * sheet over the open thread, and revealing it never opens Home.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./homeThread', () => ({ openHomeThread: vi.fn() }));

import { revealDashboard } from './dashboardNav';
import { openHomeThread } from './homeThread';
import { dashboardRailShown, setDashboardRailShown } from '../dashboardRail';
import { closeDashboardSheet, dashboardSheetOpen } from '../dashboards';
import { activeMenuItem, focusedThreadId } from '../store';
import { viewportIsMobile } from '../../utils/viewport';

describe('revealDashboard', () => {
  const wasMobile = viewportIsMobile.value;

  beforeEach(() => {
    vi.clearAllMocks();
    activeMenuItem.value = 'files';
    setDashboardRailShown(false);
    closeDashboardSheet();
    focusedThreadId.value = 'thread-a';
  });

  afterEach(() => {
    viewportIsMobile.value = wasMobile;
    focusedThreadId.value = null;
  });

  it('on desktop shows the rail and leaves the Canvas and the Home chat alone', () => {
    viewportIsMobile.value = false;
    revealDashboard();
    expect(dashboardRailShown.value).toBe(true);
    expect(dashboardSheetOpen.value).toBe(false);
    expect(activeMenuItem.value).toBe('files');
    expect(openHomeThread).not.toHaveBeenCalled();
  });

  it('on a phone opens the sheet over the current thread, not Home', () => {
    viewportIsMobile.value = true;
    revealDashboard();
    expect(dashboardSheetOpen.value).toBe(true);
    expect(dashboardRailShown.value).toBe(false);
    expect(openHomeThread).not.toHaveBeenCalled();
    expect(focusedThreadId.value).toBe('thread-a');
  });

  it('on a phone brings the bars back, since the sheet hangs below the header', () => {
    viewportIsMobile.value = true;
    const reveal = vi.fn();
    document.addEventListener('reveal-mobile-bars', reveal);
    try {
      revealDashboard();
    } finally {
      document.removeEventListener('reveal-mobile-bars', reveal);
    }
    expect(reveal).toHaveBeenCalledTimes(1);
  });
});
