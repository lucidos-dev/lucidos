import { getRemPx } from '../utils/dom';
import { DASHBOARD_CELL_REM, DASHBOARD_GAP_REM, DASHBOARD_MIN_COLUMNS, DASHBOARD_PAD_X_REM } from '../utils/dashboardGeometry';

const DRAWER_ROW_BUTTON_REM = 2.25 + 0.25;
const DRAWER_ROW_PAD_REM = 0.5;
const TITLEBAR_LIGHTS_RESERVE_PX = 80;

const THREAD_ROW_SIDE_REM = 2.25;
const THREAD_ROW_LEAD_CONTROL_REM = 2.25 + 0.25;
const THREAD_ROW_PAD_REM = 0.5;
const THREAD_ROW_CLUSTER_REM = 2 * 2.25 + 2.1;
const MIN_CONTENT_PANE_REM = 22.5;

export function computeMinDrawerWidth(remPx: number, leadPx: number): number {
  const leadEndPx = leadPx + 2 * DRAWER_ROW_BUTTON_REM * remPx;
  const trailEndPx = (DRAWER_ROW_PAD_REM + 2 * DRAWER_ROW_BUTTON_REM) * remPx;
  return Math.ceil(2 * Math.max(leadEndPx, trailEndPx));
}

function titlebarLightsReservePx(): number {
  const reserve = parseFloat(
    getComputedStyle(document.documentElement).getPropertyValue('--titlebar-lights-reserve'),
  );
  return Number.isFinite(reserve) ? reserve : TITLEBAR_LIGHTS_RESERVE_PX;
}

export function minDrawerWidth(): number {
  return computeMinDrawerWidth(getRemPx(), titlebarLightsReservePx());
}

export function computeMinThreadPaneWidth(remPx: number, leadPx: number, leadControls: number): number {
  const sideRem = THREAD_ROW_SIDE_REM + leadControls * THREAD_ROW_LEAD_CONTROL_REM;
  const sidePx = Math.max(leadPx, THREAD_ROW_PAD_REM * remPx) + sideRem * remPx;
  return Math.ceil(2 * sidePx + THREAD_ROW_CLUSTER_REM * remPx);
}

export const THREAD_ROW = '.desktop-header .pane-header-brand';

export const THREAD_ROW_LEAD_CONTROLS = `${THREAD_ROW} > :is(.brand-compose-btn, .home-thread-btn)`;

export function minThreadPanePx(): number {
  const leadControls = document.querySelectorAll(THREAD_ROW_LEAD_CONTROLS).length;
  return computeMinThreadPaneWidth(getRemPx(), titlebarLightsReservePx(), leadControls);
}

export function computeMinContentPaneWidth(remPx: number): number {
  return Math.ceil(MIN_CONTENT_PANE_REM * remPx);
}

export function minContentPanePx(): number {
  return computeMinContentPaneWidth(getRemPx());
}

export const DIVIDER_BAND_REM = 0.1875;

export function dividerBandPx(): number {
  return DIVIDER_BAND_REM * getRemPx();
}

export function computeMinDashboardRailWidth(remPx: number, scrollbarGutterPx: number): number {
  const dashboard = DASHBOARD_MIN_COLUMNS * DASHBOARD_CELL_REM + (DASHBOARD_MIN_COLUMNS - 1) * DASHBOARD_GAP_REM + 2 * DASHBOARD_PAD_X_REM;
  const rem = dashboard + DIVIDER_BAND_REM;
  return Math.ceil(rem * remPx + scrollbarGutterPx);
}

function publishedScrollbarGutterPx(): number {
  const value = getComputedStyle(document.documentElement).getPropertyValue('--scrollbar-gutter-width');
  return parseFloat(value) || 0;
}

export function minDashboardRailPx(): number {
  return computeMinDashboardRailWidth(getRemPx(), publishedScrollbarGutterPx());
}

export interface SplitBounds {
  minThreadPx: number;
  minContentPx: number;
}

export function splitBounds(): SplitBounds {
  return { minThreadPx: minThreadPanePx(), minContentPx: minContentPanePx() };
}

export interface PaneFloors extends SplitBounds {
  drawerPx: number;
  railPx: number;
}

export function paneFloors(): PaneFloors {
  return { ...splitBounds(), drawerPx: minDrawerWidth(), railPx: minDashboardRailPx() };
}
