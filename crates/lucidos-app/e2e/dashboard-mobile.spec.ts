import { test, expect, Page, Locator } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, ensureMobileView, ensureOnThreadPane, gotoWithRetry, touchPress, waitForVisibleInput, waitForWorkspaceReady } from './helpers';
import { ensureHomeThread, psql, seedChatThread } from './db-helpers';

/**
 * The phone dashboard is a panel over any thread (ADR 0538). A button beside
 * the mark in the thread header opens it, over Home or any other thread. Opening
 * it never changes focus, so the hash stays put. Closing it
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

const toggle = (page: Page) => page.locator('.mobile-thread-header .header-mark-group .dashboard-panel-toggle:visible');
const panel = (page: Page) => page.locator('.dashboard-panel');
const tile = (page: Page, appId: string): Locator => panel(page).locator(`.dashboard-tile[data-dashboard-tile^="${appId}?"]`);
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

/** Open the panel from its button and wait for the fade to finish. */
async function openPanel(page: Page): Promise<void> {
  await toggle(page).click();
  await expect(panel(page)).toBeVisible();
  await expect(panel(page)).toHaveCSS('opacity', '1');
  // The grid is inert until its tiles are seen, or the settle fuse runs out.
  await expect(panel(page).locator('.dashboard-grid.is-settling')).toHaveCount(0);
}

/** The app header's bottom edge, in px. */
const headerBottom = (page: Page) => page.evaluate(() => document.querySelector('.app-header')!.getBoundingClientRect().bottom);

/** The panel's top edge, in px. */
const panelTop = (page: Page) => page.evaluate(() => document.querySelector('.dashboard-panel')!.getBoundingClientRect().top);

test.describe('The dashboard panel on a phone', () => {
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

  test('Home opens as a plain chat, with the dashboard button in the header', async ({ page }) => {
    await openHome(page);
    await expect(page.locator('.home-pane')).toHaveCount(0);
    await expect(page.locator('[data-home-face]')).toHaveCount(0);
    await expect(panel(page)).toHaveCount(0);
  });

  test('the pressed button wears the same paint as Filter on the header bar', async ({ page }) => {
    await openHome(page);
    await openPanel(page);
    await page.mouse.move(0, 0);
    await expect(toggle(page)).toHaveAttribute('aria-pressed', 'true');

    // Filter is a framed toggle, so a framed probe on the same bar is its paint.
    // Polled, because the chip fades in under a transition.
    const paints = () => page.evaluate(() => {
      const paint = (el: Element) => {
        const style = getComputedStyle(el);
        return { color: style.color, background: style.backgroundColor };
      };
      const probe = document.createElement('button');
      probe.className = 'icon-btn header-icon';
      probe.dataset.tone = 'framed';
      probe.setAttribute('aria-pressed', 'true');
      document.querySelector('.mobile-thread-header')!.append(probe);
      const filter = paint(probe);
      probe.remove();
      return { filter, dashboard: paint(document.querySelector('.mobile-thread-header .dashboard-panel-toggle')!) };
    });
    await expect.poll(async () => (await paints()).dashboard).toEqual((await paints()).filter);
    expect((await paints()).dashboard.background, 'a pressed toggle wears a chip').not.toBe('rgba(0, 0, 0, 0)');
  });

  test('the button opens the panel over Home, flush below the header, and the hash stays put', async ({ page }) => {
    await addToDashboard(page, a);
    await openHome(page);
    const hash = await page.evaluate(() => location.hash);

    await openPanel(page);
    await expect(tile(page, a)).toBeVisible();
    expect(await page.evaluate(() => location.hash)).toBe(hash);
    expect(await panelTop(page)).toBeCloseTo(await headerBottom(page), 0);
  });

  for (const [name, title] of [
    ['a short title', 'E2E short'],
    ['a long title', 'E2E a long thread title that has to wrap onto a second line on a narrow phone screen'],
  ] as const) {
    test(`over a thread with ${name}, the panel meets the header, and focus stays`, async ({ page }) => {
      const threadId = seedChatThread({ turns: 1, needles: [], title });
      try {
        await addToDashboard(page, a);
        await gotoWithRetry(page, `/#thread=${threadId}`);
        await ensureOnThreadPane(page);
        await expect(toggle(page)).toBeVisible({ timeout: 15_000 });

        const hash = await page.evaluate(() => location.hash);
        await openPanel(page);
        expect(await panelTop(page)).toBeCloseTo(await headerBottom(page), 0);
        expect(await page.evaluate(() => location.hash)).toBe(hash);
        expect(await page.evaluate(() => localStorage.getItem('lucidos-focused-thread'))).toBe(threadId);
      } finally {
        psql(`DELETE FROM events WHERE thread_id = '${threadId}'; DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`);
      }
    });
  }

  test('the button stays tappable and closes the panel', async ({ page }) => {
    await openHome(page);
    await openPanel(page);
    await expect(toggle(page)).toHaveAttribute('aria-pressed', 'true');

    await toggle(page).click();
    await expect(panel(page)).toBeHidden();
    await expect(toggle(page)).toHaveAttribute('aria-pressed', 'false');
  });

  test('the open panel hides the thread chevrons, and the group beside them holds still', async ({ page }) => {
    await openHome(page);
    const chevrons = page.locator('.mobile-thread-header .header-nav-cluster > .nav-chevron');
    await expect(chevrons).toHaveCount(2);
    for (const chevron of await chevrons.all()) await expect(chevron).toBeVisible();
    const toggleX = () => toggle(page).evaluate((el) => el.getBoundingClientRect().left);
    const before = await toggleX();

    await openPanel(page);
    for (const chevron of await chevrons.all()) await expect(chevron).toBeHidden();
    expect(await toggleX()).toBeCloseTo(before, 0);

    await toggle(page).click();
    for (const chevron of await chevrons.all()) await expect(chevron).toBeVisible();
  });

  /** Invariant: a pane swipe that starts on the open panel moves the strip. */
  test('a pane swipe from the open panel moves the strip, and the panel is open on return', async ({ page, browserName }) => {
    test.skip(browserName === 'webkit', 'CDP touch input is Chromium only');
    await openHome(page);
    await openPanel(page);
    const { width, height } = page.viewportSize()!;
    const y = height / 2;

    await touchPress(page, [
      { type: 'touchStart', x: width * 0.4, y },
      { type: 'touchMove', x: width * 0.55, y },
      { type: 'touchMove', x: width * 0.7, y },
      { type: 'touchMove', x: width * 0.85, y },
      { type: 'touchEnd' },
    ]);
    await expect(page.locator('.app-header')).toHaveAttribute('data-mobile-view', 'threads');

    await ensureMobileView(page, 'thread');
    await expect(toggle(page)).toHaveAttribute('aria-pressed', 'true');
    await expect(panel(page)).toBeVisible();
  });

  /** Invariant: the header stays live over the panel, so the mark opens the Lucidos menu. */
  test('the Lucidos mark opens its menu over the open panel', async ({ page }) => {
    await openHome(page);
    await openPanel(page);

    await page.locator('.mobile-thread-header [data-role="brand-menu-toggle"]:visible').first().click();
    await expect(page.locator('.brand-menu')).toBeVisible();
    await expect(toggle(page)).toHaveAttribute('aria-pressed', 'true');
  });

  /** Invariant: closing and reopening the panel never reloads a tile. */
  test('a tile is not reloaded when the panel closes and opens again', async ({ page }) => {
    await addToDashboard(page, a);
    await openHome(page);
    await openPanel(page);
    const loaded = await frame(page, a).locator('#load').textContent();

    await toggle(page).click();
    await expect(panel(page)).toBeHidden();
    await openPanel(page);
    await expect(frame(page, a).locator('#load')).toHaveText(loaded ?? '');
  });

  /** Invariant: each dashboard's tiles are independent (ADR 0496). */
  test('the phone dashboard keeps its own tiles and arrangement, and leaves desktop alone', async ({ page }) => {
    const desktopBefore = await dashboardTiles(page, 'desktop');
    await addToDashboard(page, a);
    await addToDashboard(page, b);
    await arrange(page, 'phone', [{ app_id: b, size: 'small' }, { app_id: a, size: 'medium' }]);
    await openHome(page);
    await openPanel(page);

    expect(await order(page, b)).toBeLessThan(await order(page, a));
    await expect(tile(page, b)).toHaveAttribute('data-size', 'small');

    await panel(page).locator('.dashboard-edit').click();
    await tile(page, a).locator('.dashboard-size-pill button[aria-label="small"]').click();
    await expect.poll(async () => (await dashboardTiles(page, 'phone')).find((t) => t.app_id === a)?.size).toBe('small');
    expect(await dashboardTiles(page, 'desktop')).toEqual(desktopBefore);
  });

  /** Invariant: a phone narrower than two cells shrinks them to fit two
   *  columns, and never overflows the dashboard. */
  test('a medium tile fills the two columns inside the dashboard, with no overflow', async ({ page }) => {
    await addToDashboard(page, a);
    await openHome(page);
    await openPanel(page);
    const dashboard = panel(page).locator('.dashboard');
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
    await openPanel(page);
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
    await expect(panel(page).locator('.dashboard-tile-shield')).toHaveCount(0);
    expect([await order(page, a), await order(page, b)]).toEqual(before);

    const cdp = await page.context().newCDPSession(page);
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [from] });
    await page.waitForTimeout(800);
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    // One press opens one menu, and Edit dashboard in it enters edit mode.
    const menu = page.locator('.thread-overflow-menu');
    await expect(menu).toHaveCount(1);
    await expect(panel(page).locator('.dashboard-tile-shield')).toHaveCount(0);
    await menu.getByRole('menuitem', { name: 'Edit dashboard' }).click();
    await expect(panel(page).locator('.dashboard-tile-shield')).toHaveCount(2);
  });
});
