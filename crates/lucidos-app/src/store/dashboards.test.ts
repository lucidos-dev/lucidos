import { beforeEach, describe, expect, it } from 'vitest';
import {
  closeDashboardPanel,
  dashboardPanelOpen,
  openDashboardPanel,
} from './dashboards';
import { toggleDashboardPanel } from './actions/thread-navigation';
import { focusedThreadId } from './store';

describe('the dashboard panel', () => {
  beforeEach(() => {
    closeDashboardPanel();
    focusedThreadId.value = 'thread-a';
  });

  it('opens, closes and toggles', () => {
    expect(dashboardPanelOpen.value).toBe(false);
    openDashboardPanel();
    expect(dashboardPanelOpen.value).toBe(true);
    toggleDashboardPanel();
    expect(dashboardPanelOpen.value).toBe(false);
    toggleDashboardPanel();
    expect(dashboardPanelOpen.value).toBe(true);
    closeDashboardPanel();
    expect(dashboardPanelOpen.value).toBe(false);
  });

  it('never moves the focused thread', () => {
    openDashboardPanel();
    toggleDashboardPanel();
    toggleDashboardPanel();
    closeDashboardPanel();
    expect(focusedThreadId.value).toBe('thread-a');
  });
});
