
import { signal } from '@preact/signals';
import { minDashboardRailPx } from './paneMinimums';
import { DASHBOARD_RAIL_SHOWN_KEY, DASHBOARD_RAIL_WIDTH_KEY } from './dashboardRailKeys';

export const dashboardRailShown = signal<boolean>(localStorage.getItem(DASHBOARD_RAIL_SHOWN_KEY) !== 'false');

export const dashboardRailWidth = signal<number>(
  Math.max(minDashboardRailPx(), Number(localStorage.getItem(DASHBOARD_RAIL_WIDTH_KEY)) || 0),
);

export function setDashboardRailShown(shown: boolean): void {
  dashboardRailShown.value = shown;
  localStorage.setItem(DASHBOARD_RAIL_SHOWN_KEY, String(shown));
}

export function persistDashboardRailWidth(): void {
  localStorage.setItem(DASHBOARD_RAIL_WIDTH_KEY, String(dashboardRailWidth.value));
}

export function clampDashboardRailWidth(): void {
  const min = minDashboardRailPx();
  if (dashboardRailWidth.value >= min) return;
  dashboardRailWidth.value = min;
  persistDashboardRailWidth();
}
