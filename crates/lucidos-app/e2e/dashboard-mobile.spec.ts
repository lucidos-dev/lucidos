import { test, expect, Page, Locator } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, ensureOnThreadPane, gotoWithRetry, touchPress, waitForVisibleInput, waitForWorkspaceReady } from './helpers';
import { ensureHomeThread, psql, seedChatThread } from './db-helpers';

/**
 * The phone dashboard is a sheet over any thread (ADR 0538). A button at the
 * end of the thread title row opens it, over Home or any other thread. Opening
 * it never changes focus, so the hash and the title row stay put. Closing it
 * never reloads a tile. The phone dashboard has its own tiles, apart from the
 * desktop one.
 */

let written: string[] = [];
let added: string[] = [];

async function writeFile(page: Page, path: string, body: string): Promise<void> {
  written.push(path);
  const resp = await apiRequest(page).put(`/api/v1/data/${path}`, {
    headers: { 'Content-Type': 'text/plain' },
    data: body,
  });
  expect(resp.ok(), `PUT ${path}`).toBeTruthy();
}

/** A widget that shows a random number per load, so a reload is visible. */
const LIVE_WIDGET = '<!doctype html><html><head><script src="/api/v1/sdk.js"></script></head><body>'
  + '<p id="load"></p><script>document.getElementById("load").textContent = String(Math.random());</script></body></html>';

/** A reusable widget, as a tile on the phone dashboard. */
async function addToDashboard(page: Page, appId: string, manifest: Record<string, unknown> = {}): Promise<void> {
  await writeFile(page, `apps/${appId}/index.html`, LIVE_WIDGET);
  await writeFile(page, `apps/${appId}/manifest.json`, JSON.stringify({
    name: appId, description: 'E2E dashboard tile', kind: 'widget', reusable: true, sizes: ['medium', 'small'], ...manifest,
  }));
  const resp = await apiRequest(page).post('/api/v1/widgets/dashboard-add', { data: { app_id: appId, dashboard: 'phone' } });
  expect(resp.ok(), `add ${appId}: ${await resp.text()}`).toBeTruthy();
  added.push(appId);
}

async function arrange(page: Page, dashboard: 'phone' | 'desktop', tiles: Array<Record<string, unknown>>): Promise<void> {
  const resp = await apiRequest(page).post('/api/v1/widgets/arrange', { data: { dashboard, tiles } });
  expect(resp.ok(), `arrange ${dashboard}: ${await resp.text()}`).toBeTruthy();
}

async function dashboardTiles(page: Page, dashboard: 'phone' | 'desktop'): Promise<Array<{ app_id: string; size: string }>> {
  const resp = await page.request.get(`/api/v1/dashboard?dashboard=${dashboard}`);
  return (await resp.json()).tiles;
}

const toggle = (page: Page) => page.locator('.mobile-thread-title-row:visible .dashboard-sheet-toggle');
const sheet = (page: Page) => page.locator('.dashboard-sheet-panel');
const tile = (page: Page, appId: string): Locator => sheet(page).locator(`.dashboard-tile[data-dashboard-tile^="${appId}?"]`);
const frame = (page: Page, appId: string) => tile(page, appId).frameLocator('iframe');
const order = (page: Page, appId: string) => tile(page, appId).evaluate((el) => Number((el as HTMLElement).style.order));

/** A cold start with no stored focus lands on Home, a plain chat. */
async function openHome(page: Page): Promise<void> {
  await page.addInitScript(() => localStorage.removeItem('lucidos-focused-thread'));
  await gotoWithRetry(page, '/');
  await ensureOnThreadPane(page);
  await waitForVisibleInput(page);
  await waitForWorkspaceReady(page);
  await expect(toggle(page)).toBeVisible({ timeout: 15_000 });
}

/** Open the sheet from its button and wait for the rise to finish. */
async function openSheet(page: Page): Promise<void> {
  await toggle(page).click();
  await expect(sheet(page)).toBeVisible();
  await expect(sheet(page)).toHaveCSS('transform', 'none');
}

/** The visible title row's top and bottom, in px. */
const rowEdges = (page: Page) => page.evaluate(() => {
  const row = [...document.querySelectorAll('.mobile-thread-title-row')]
    .find((r) => r.getBoundingClientRect().height > 0)!.getBoundingClientRect();
  return { top: row.top, bottom: row.bottom };
});

/** The sheet's top edge, and the 0.75rem gap it keeps below the title row, in px. */
const sheetEdges = (page: Page) => page.evaluate(() => ({
  top: document.querySelector('.dashboard-sheet-panel')!.getBoundingClientRect().top,
  gap: parseFloat(getComputedStyle(document.documentElement).fontSize) * 0.75,
}));

test.describe('The dashboard sheet on a phone', () => {
  let a: string;
  let b: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // An earlier spec in the shard may have truncated the Home row.
    ensureHomeThread();
    written = [];
    added = [];
    const tag = randomUUID().slice(0, 8);
    a = `e2e-hp-a-${tag}`;
    b = `e2e-hp-b-${tag}`;
    await page.goto('/');
  });

  test.afterEach(async ({ page }) => {
    for (const appId of added) {
      await apiRequest(page).post('/api/v1/widgets/dashboard-remove', { data: { app_id: appId, dashboard: 'phone' } });
    }
    for (const path of written) await apiRequest(page).delete(`/api/v1/data/${path}`);
  });

  test('Home opens as a plain chat, with the dashboard button in its title row', async ({ page }) => {
    await openHome(page);
    await expect(page.locator('.home-pane')).toHaveCount(0);
    await expect(page.locator('[data-home-face]')).toHaveCount(0);
    await expect(sheet(page)).toHaveCount(0);
  });

  test('the button opens the sheet over Home, and the hash and title row stay put', async ({ page }) => {
    await addToDashboard(page, a);
    await openHome(page);
    const hash = await page.evaluate(() => location.hash);
    const rowBefore = await rowEdges(page);

    await openSheet(page);
    await expect(tile(page, a)).toBeVisible();
    expect(await page.evaluate(() => location.hash)).toBe(hash);
    const row = await rowEdges(page);
    expect(row.top).toBeCloseTo(rowBefore.top, 0);
    expect(row.bottom).toBeCloseTo(rowBefore.bottom, 0);
    const sheet = await sheetEdges(page);
    expect(sheet.top).toBeCloseTo(row.bottom + sheet.gap, 0);
  });

  for (const [name, title] of [
    ['a short title', 'E2E short'],
    ['a long title', 'E2E a long thread title that has to wrap onto a second line on a narrow phone screen'],
  ] as const) {
    test(`over a thread with ${name}, the sheet hangs from the title row, and focus stays`, async ({ page }) => {
      const threadId = seedChatThread({ turns: 1, needles: [], title });
      try {
        await addToDashboard(page, a);
        await gotoWithRetry(page, `/#thread=${threadId}`);
        await ensureOnThreadPane(page);
        await expect(toggle(page)).toBeVisible({ timeout: 15_000 });

        const hash = await page.evaluate(() => location.hash);
        await openSheet(page);
        const row = await rowEdges(page);
        const sheet = await sheetEdges(page);
        expect(sheet.top).toBeCloseTo(row.bottom + sheet.gap, 0);
        expect(await page.evaluate(() => location.hash)).toBe(hash);
        expect(await page.evaluate(() => localStorage.getItem('lucidos-focused-thread'))).toBe(threadId);
      } finally {
        psql(`DELETE FROM events WHERE thread_id = '${threadId}'; DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`);
      }
    });
  }

  test('the button stays tappable and closes the sheet', async ({ page }) => {
    await openHome(page);
    await openSheet(page);
    await expect(toggle(page)).toHaveAttribute('aria-pressed', 'true');

    await toggle(page).click();
    await expect(sheet(page)).toBeHidden();
    await expect(toggle(page)).toHaveAttribute('aria-pressed', 'false');
  });

  test('a tap on the dim above the sheet closes it', async ({ page }) => {
    await openHome(page);
    await openSheet(page);
    const row = await rowEdges(page);
    const { gap } = await sheetEdges(page);

    await page.mouse.click(8, row.bottom + gap / 2);
    await expect(sheet(page)).toBeHidden();
  });

  /** Invariant: closing and reopening the sheet never reloads a tile. */
  test('a tile is not reloaded when the sheet closes and opens again', async ({ page }) => {
    await addToDashboard(page, a);
    await openHome(page);
    await openSheet(page);
    const loaded = await frame(page, a).locator('#load').textContent();

    await toggle(page).click();
    await expect(sheet(page)).toBeHidden();
    await openSheet(page);
    await expect(frame(page, a).locator('#load')).toHaveText(loaded ?? '');
  });

  /** Invariant: each dashboard's tiles are independent (ADR 0496). */
  test('the phone dashboard keeps its own tiles and arrangement, and leaves desktop alone', async ({ page }) => {
    const desktopBefore = await dashboardTiles(page, 'desktop');
    await addToDashboard(page, a);
    await addToDashboard(page, b);
    await arrange(page, 'phone', [{ app_id: b, size: 'small' }, { app_id: a, size: 'medium' }]);
    await openHome(page);
    await openSheet(page);

    expect(await order(page, b)).toBeLessThan(await order(page, a));
    await expect(tile(page, b)).toHaveAttribute('data-size', 'small');

    await sheet(page).locator('.dashboard-edit').click();
    await tile(page, a).locator('.dashboard-size-pill button[aria-label="small"]').click();
    await expect.poll(async () => (await dashboardTiles(page, 'phone')).find((t) => t.app_id === a)?.size).toBe('small');
    expect(await dashboardTiles(page, 'desktop')).toEqual(desktopBefore);
  });

  /** Invariant: a phone narrower than two cells shrinks them to fit two
   *  columns, and never overflows the dashboard. */
  test('a medium tile fills the two columns inside the dashboard, with no overflow', async ({ page }) => {
    await addToDashboard(page, a);
    await openHome(page);
    await openSheet(page);
    const dashboard = sheet(page).locator('.dashboard');
    await expect(dashboard).toHaveCSS('--dashboard-columns', '2');
    const content = await dashboard.evaluate((el) => {
      const r = el.getBoundingClientRect();
      const s = getComputedStyle(el);
      return { left: r.left + parseFloat(s.paddingLeft), right: r.right - parseFloat(s.paddingRight) };
    });
    const t = (await tile(page, a).boundingBox())!;
    expect(t.x).toBeGreaterThanOrEqual(content.left - 1);
    expect(t.x + t.width).toBeLessThanOrEqual(content.right + 1);
    expect(Math.abs(t.x - content.left - (content.right - t.x - t.width))).toBeLessThanOrEqual(1);
    expect(await dashboard.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
  });

  test('a finger drag over a live tile moves nothing, and a hold opens the tile menu', async ({ page, browserName }) => {
    test.skip(browserName === 'webkit', 'CDP touch input is Chromium only');
    await addToDashboard(page, a);
    await addToDashboard(page, b);
    await openHome(page);
    await openSheet(page);
    // A frame forwards a hold only once the host's push marked it a dashboard
    // tile with a menu. The place stamp lands in the same handler as that mark,
    // and a slow renderer start on a runner can outlast the 450 ms hold.
    for (const id of [a, b]) {
      await expect(frame(page, id).locator('html')).toHaveAttribute('data-widget-place', 'dashboard');
    }
    const before = [await order(page, a), await order(page, b)];
    const boxA = (await tile(page, a).boundingBox())!;
    const boxB = (await tile(page, b).boundingBox())!;
    const from = { x: boxA.x + boxA.width / 2, y: boxA.y + boxA.height / 2 };
    const to = { x: boxB.x + boxB.width / 2, y: boxB.y + boxB.height / 2 };

    await touchPress(page, [
      { type: 'touchStart', ...from },
      { type: 'touchMove', x: from.x, y: (from.y + to.y) / 2 },
      { type: 'touchMove', ...to },
      { type: 'touchEnd' },
    ]);
    await expect(sheet(page).locator('.dashboard-tile-shield')).toHaveCount(0);
    expect([await order(page, a), await order(page, b)]).toEqual(before);

    const cdp = await page.context().newCDPSession(page);
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [from] });
    await page.waitForTimeout(800);
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    // One press opens one menu, and Edit dashboard in it enters edit mode.
    const menu = page.locator('.thread-overflow-menu');
    await expect(menu).toHaveCount(1);
    await expect(sheet(page).locator('.dashboard-tile-shield')).toHaveCount(0);
    await menu.getByRole('menuitem', { name: 'Edit dashboard' }).click();
    await expect(sheet(page).locator('.dashboard-tile-shield')).toHaveCount(2);
  });
});
