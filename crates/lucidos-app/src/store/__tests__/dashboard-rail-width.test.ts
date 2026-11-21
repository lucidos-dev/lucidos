/**
 * The desktop rail that holds the dashboard: its floor is two dashboard cells, a
 * drag stops at both walls, and the width and shown state persist per device.
 * The width it draws at is `computeDrawnLayout`'s (splitHelpers.test.ts).
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import { DASHBOARD_CELL_REM, DASHBOARD_GAP_REM, DASHBOARD_PAD_X_REM, dashboardColumns, dashboardContentWidth } from '../../utils/dashboardGeometry';
import { dashboardRailCeiling } from '../../components/layout/splitHelpers';
import { DIVIDER_BAND_REM, computeMinDashboardRailWidth } from '../paneMinimums';
import { DASHBOARD_RAIL_SHOWN_KEY, DASHBOARD_RAIL_WIDTH_KEY } from '../dashboardRailKeys';
import { dashboardRailShown, dashboardRailWidth, persistDashboardRailWidth, setDashboardRailShown } from '../dashboardRail';

describe('the rail floor', () => {
  it('fits exactly two dashboard cells at every UI scale, beside any scrollbar gutter', () => {
    for (const rem of [16, 18, 20, 24, 28]) {
      for (const gutter of [0, 8, 15]) {
        const floor = computeMinDashboardRailWidth(rem, gutter);
        const content = dashboardContentWidth(floor - DIVIDER_BAND_REM * rem - gutter, rem);
        expect(dashboardColumns(content, rem)).toBe(2);
        expect(content).toBeGreaterThanOrEqual((2 * DASHBOARD_CELL_REM + DASHBOARD_GAP_REM) * rem);
        expect(content).toBeLessThan((2 * DASHBOARD_CELL_REM + DASHBOARD_GAP_REM) * rem + 1);
      }
    }
  });

  it('grows with the root font size', () => {
    expect(computeMinDashboardRailWidth(20, 0)).toBeGreaterThan(computeMinDashboardRailWidth(16, 0));
  });

  it('keeps the rail body\'s scrollbar gutter reserved, which the floor counts', () => {
    const css = readFileSync(new URL('../../styles/panels/dashboard.css', import.meta.url), 'utf8');
    expect(/\.dashboard-rail-body \{[^}]*scrollbar-gutter: stable/.test(css)).toBe(true);
  });

  it("mirrors the dashboard's side padding in CSS", () => {
    const css = readFileSync(new URL('../../styles/panels/dashboard.css', import.meta.url), 'utf8');
    const dashboard = /\.dashboard \{[^}]*padding: var\(--space-md\) var\(--space-lg\)/.exec(css);
    expect(dashboard, '.dashboard pads its sides with --space-lg, which DASHBOARD_PAD_X_REM restates').not.toBeNull();
    expect(DASHBOARD_PAD_X_REM).toBe(1);
  });

  it("mirrors the divider band the rail pads its leading side by", () => {
    const rail = readFileSync(new URL('../../styles/panels/dashboard.css', import.meta.url), 'utf8');
    expect(/\.dashboard-rail \{[^}]*padding-left: var\(--divider-width\)/.test(rail)).toBe(true);
    const base = readFileSync(new URL('../../styles/global/base.css', import.meta.url), 'utf8');
    expect(base).toContain(`--divider-width: ${DIVIDER_BAND_REM}rem;`);
  });
});

describe('dashboardRailCeiling', () => {
  const bounds = { minThreadPx: 400, minContentPx: 360 };

  it('takes a wider rail out of the Canvas while the Conversation holds its width', () => {
    expect(dashboardRailCeiling(1600, 500, 0.4, bounds)).toBe(1600 - 500 - 360);
  });

  it('takes it out of the one pane left when the other is collapsed', () => {
    expect(dashboardRailCeiling(1600, 0, 0, bounds)).toBe(1600 - 360);
    expect(dashboardRailCeiling(1600, 1200, 1, bounds)).toBe(1600 - 400);
  });
});

describe('the rail store', () => {
  it('persists the shown state per device', () => {
    const before = dashboardRailShown.value;
    setDashboardRailShown(!before);
    expect(localStorage.getItem(DASHBOARD_RAIL_SHOWN_KEY)).toBe(String(!before));
    setDashboardRailShown(before);
    expect(dashboardRailShown.value).toBe(before);
  });

  it('persists the width', () => {
    dashboardRailWidth.value = 512;
    persistDashboardRailWidth();
    expect(localStorage.getItem(DASHBOARD_RAIL_WIDTH_KEY)).toBe('512');
  });
});
