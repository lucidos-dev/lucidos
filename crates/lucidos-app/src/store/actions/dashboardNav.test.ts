/**
 * The Home icon opens the Home chat; on desktop the dashboard lives in its rail.
 * The phone keeps one pane with two faces.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./homeThread', () => ({ openHomeThread: vi.fn() }));
vi.mock('../dashboards', () => ({ landNextHomeOnDashboard: vi.fn() }));

import { openHome, revealDashboard } from './dashboardNav';
import { openHomeThread } from './homeThread';
import { landNextHomeOnDashboard } from '../dashboards';
import { dashboardRailShown, setDashboardRailShown } from '../dashboardRail';
import { activeMenuItem, focusedThreadId } from '../store';
import { viewportIsMobile } from '../../utils/viewport';

describe('openHome', () => {
  const wasMobile = viewportIsMobile.value;

  beforeEach(() => {
    vi.clearAllMocks();
    activeMenuItem.value = 'files';
  });

  afterEach(() => {
    viewportIsMobile.value = wasMobile;
  });

  it('on desktop opens the Home chat and leaves the Canvas alone', () => {
    viewportIsMobile.value = false;
    openHome();
    expect(openHomeThread).toHaveBeenCalledTimes(1);
    expect(activeMenuItem.value).toBe('files');
    expect(landNextHomeOnDashboard).not.toHaveBeenCalled();
  });

  it('on desktop in Home, leaves a hidden rail hidden: only its toggle shows it', () => {
    viewportIsMobile.value = false;
    setDashboardRailShown(false);
    focusedThreadId.value = 'home-1';
    openHome();
    expect(dashboardRailShown.value).toBe(false);
    expect(openHomeThread).toHaveBeenCalledTimes(1);
    focusedThreadId.value = null;
  });

  it('on a phone lands Home on its dashboard face', () => {
    viewportIsMobile.value = true;
    openHome();
    expect(landNextHomeOnDashboard).toHaveBeenCalledTimes(1);
    expect(openHomeThread).toHaveBeenCalledTimes(1);
  });
});

describe('revealDashboard', () => {
  const wasMobile = viewportIsMobile.value;

  beforeEach(() => {
    vi.clearAllMocks();
    activeMenuItem.value = 'files';
    setDashboardRailShown(false);
  });

  afterEach(() => {
    viewportIsMobile.value = wasMobile;
  });

  it('on desktop shows the rail and leaves the Canvas and the Home chat alone', () => {
    viewportIsMobile.value = false;
    revealDashboard();
    expect(dashboardRailShown.value).toBe(true);
    expect(activeMenuItem.value).toBe('files');
    expect(openHomeThread).not.toHaveBeenCalled();
  });

  it("on a phone lands on Home's dashboard face", () => {
    viewportIsMobile.value = true;
    revealDashboard();
    expect(landNextHomeOnDashboard).toHaveBeenCalledTimes(1);
    expect(openHomeThread).toHaveBeenCalledTimes(1);
  });
});
