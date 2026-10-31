/** The desktop rail that holds the dashboard: whether it shows and how wide
 *  it is. Both are per device, in `localStorage`, like the drawer's width. */

import { signal } from '@preact/signals';
import { minDashboardRailPx } from './paneMinimums';
import { DASHBOARD_RAIL_SHOWN_KEY, DASHBOARD_RAIL_WIDTH_KEY } from './dashboardRailKeys';

/** Shown unless this device hid it. */
export const dashboardRailShown = signal<boolean>(localStorage.getItem(DASHBOARD_RAIL_SHOWN_KEY) !== 'false');

/** Never below the floor: a width saved under a smaller UI scale would clip
 *  the dashboard's two columns. */
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

/** Re-apply the floor after a UI-scale change, as the drawer's width does. */
export function clampDashboardRailWidth(): void {
  const min = minDashboardRailPx();
  if (dashboardRailWidth.value >= min) return;
  dashboardRailWidth.value = min;
  persistDashboardRailWidth();
}
