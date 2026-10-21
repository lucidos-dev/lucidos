import { beforeEach, describe, expect, it } from 'vitest';
import {
  closeDashboardSheet,
  dashboardSheetOpen,
  openDashboardSheet,
  toggleDashboardSheet,
} from './dashboards';
import { focusedThreadId } from './store';

describe('the dashboard sheet', () => {
  beforeEach(() => {
    closeDashboardSheet();
    focusedThreadId.value = 'thread-a';
  });

  it('opens, closes and toggles', () => {
    expect(dashboardSheetOpen.value).toBe(false);
    openDashboardSheet();
    expect(dashboardSheetOpen.value).toBe(true);
    toggleDashboardSheet();
    expect(dashboardSheetOpen.value).toBe(false);
    toggleDashboardSheet();
    expect(dashboardSheetOpen.value).toBe(true);
    closeDashboardSheet();
    expect(dashboardSheetOpen.value).toBe(false);
  });

  it('never moves the focused thread', () => {
    openDashboardSheet();
    toggleDashboardSheet();
    toggleDashboardSheet();
    closeDashboardSheet();
    expect(focusedThreadId.value).toBe('thread-a');
  });
});
