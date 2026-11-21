/**
 * An Ongoing tile keeps its count, breakdown and icon inside its own box. The
 * breakdown sits beside the count while there is room, and wraps under it when
 * the tile is too narrow for all three. The drawer's floor at a large ui-scale
 * is the narrowest a tile gets: at 200% the icon used to land past the tile's
 * right edge.
 */
import { test, expect, type Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';
import { DRAWER_GROUPING_KEY, SELECTED_ONGOING_GROUP_KEY, THREAD_DRAWER_WIDTH_KEY } from '../src/store/drawerKeys';
import { DEVICE_ID_KEY } from '../src/utils/deviceIdKey';

/** Enough running threads for a two-digit "12 running", the widest one-line breakdown. */
const RUNNING_COUNT = 12;
const UI_SCALE = '200';

interface TileFit {
  label: string;
  overflowPx: number;
  breakdownWrapped: boolean;
}

/** Each tile's worst overflow past its content edge, and whether its breakdown
 *  starts below its count. */
async function tileFits(page: Page): Promise<TileFit[]> {
  return page.locator('.drawer-ongoing-tiles .ongoing-tile').locator('visible=true').evaluateAll(tiles => tiles.map(tile => {
    const edge = tile.getBoundingClientRect().right - parseFloat(getComputedStyle(tile).paddingRight);
    const parts = ['.ongoing-tile-count', '.ongoing-tile-breakdown', '.ongoing-tile-icon']
      .map(s => tile.querySelector(s)?.getBoundingClientRect())
      .filter((r): r is DOMRect => r !== undefined);
    const count = tile.querySelector('.ongoing-tile-count')!.getBoundingClientRect();
    const breakdown = tile.querySelector('.ongoing-tile-breakdown')?.getBoundingClientRect();
    return {
      label: tile.querySelector('.ongoing-tile-label')!.textContent ?? '',
      overflowPx: Math.max(0, ...parts.map(r => r.right - edge)),
      breakdownWrapped: breakdown !== undefined && breakdown.top >= count.bottom - 0.5,
    };
  }));
}

async function openOngoing(page: Page, drawerWidth: number, request: import('@playwright/test').APIRequestContext): Promise<void> {
  const deviceId = `e2e-ongoing-fit-${Date.now()}`;
  const seeds: [string, string][] = [
    [DEVICE_ID_KEY, deviceId],
    [DRAWER_GROUPING_KEY, 'ongoing'],
    [SELECTED_ONGOING_GROUP_KEY, 'in-flight'],
    [THREAD_DRAWER_WIDTH_KEY, String(drawerWidth)],
  ];
  await page.context().addInitScript((entries) => {
    for (const [key, value] of entries) localStorage.setItem(key, value);
  }, seeds);
  const put = await request.put('/api/v1/preferences?key=ui-scale', { data: { value: UI_SCALE, device_id: deviceId } });
  expect(put.ok()).toBe(true);
  await navigateToApp(page);
  await openThreadDrawer(page);
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--user-ui-scale').trim()))
    .toBe(`${UI_SCALE}%`);
  await expect(page.locator('.ongoing-tile-breakdown').locator('visible=true')).toHaveText(`${RUNNING_COUNT} running`);
}

test.describe('Ongoing tile fit', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearAllThreads();
    const base = Date.now();
    const ids = Array.from({ length: RUNNING_COUNT }, () => randomUUID());
    psql([
      ...ids.map((id, i) => seedThreadRow({ id, title: `running-${i}-${base}`, now: new Date(base - i * 60_000).toISOString(), archiveState: 'inbox' })),
      `UPDATE thread_summaries SET status = 'running' WHERE thread_id IN (${ids.map(id => `'${id}'`).join(', ')})`,
    ].join(';\n'));
  });

  test.afterEach(() => {
    psql(`DELETE FROM preferences WHERE key = 'ui-scale'`);
  });

  test('at the drawer floor, the breakdown wraps under the count and nothing leaves the tile', async ({ page, request }) => {
    // Below the floor, so the load clamps it to exactly the floor.
    await openOngoing(page, 1, request);
    const fits = await tileFits(page);
    for (const fit of fits) expect(fit.overflowPx, `${fit.label} overflows its tile`).toBeLessThanOrEqual(0.5);
    expect(fits.find(f => f.label === 'In flight')?.breakdownWrapped).toBe(true);
  });

  test('with room to spare, the breakdown stays beside the count', async ({ page, request }) => {
    await page.setViewportSize({ width: 1800, height: 1000 });
    await openOngoing(page, 760, request);
    const fits = await tileFits(page);
    for (const fit of fits) expect(fit.overflowPx, `${fit.label} overflows its tile`).toBeLessThanOrEqual(0.5);
    expect(fits.find(f => f.label === 'In flight')?.breakdownWrapped).toBe(false);
  });
});
