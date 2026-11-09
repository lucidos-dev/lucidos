import { test, expect, Page, Locator } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, ensureOnThreadPane, isMobileViewport, navigateToApp, openWidgetCardMenu, waitForDashboardReady } from './helpers';
import { psql } from './db-helpers';

/**
 * A widget renders in one box everywhere a size applies: its thread card and
 * its dashboard tile (the widget box). At one size the two boxes match, they
 * clip alike, the widget is told the same size, and its text computes to the
 * same px. Checked at 100% and 175% UI scale, on desktop and phone.
 *
 * The fixture is written to the old inline contract: natural height, three
 * cards in an auto-fit row. That is the widget that fit its card and clipped
 * in its tile.
 */

const SIZES = ['small', 'medium', 'large'] as const;
type Size = typeof SIZES[number];

const OLD_CONTRACT_WIDGET = '<!doctype html><html><head><script src="/api/v1/sdk.js"></script>'
  + '<style>body{margin:0;padding:0.75rem;font-size:1rem}'
  + '.row{display:grid;grid-template-columns:repeat(auto-fit,minmax(7rem,1fr));gap:0.5rem}'
  + '.day{border:1px solid currentColor;padding:0.5rem}h2{margin:0 0 0.25rem;font-size:1.125rem}</style></head>'
  + '<body><div class="row">'
  + '<div class="day"><h2 id="first">Tuesday</h2><p>Easy 8 km</p></div>'
  + '<div class="day"><h2>Wednesday</h2><p>Tempo 10 km</p></div>'
  + '<div class="day"><h2>Thursday</h2><p>Rest</p></div>'
  + '</div><button>Skip</button></body></html>';

let written: string[] = [];

async function writeFile(page: Page, path: string, body: string): Promise<void> {
  written.push(path);
  const resp = await apiRequest(page).put(`/api/v1/data/${path}`, {
    headers: { 'Content-Type': 'text/plain' },
    data: body,
  });
  expect(resp.ok(), `PUT ${path}`).toBeTruthy();
}

function seedThreadShowingWidget(threadId: string, appId: string): void {
  const now = new Date().toISOString();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) `
      + `VALUES ('${threadId}', 'E2E one widget box', 'chat', '${now}', 2, false, true, 'idle', 'inbox', false, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
      + `VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"plan my runs","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
      + `VALUES ('${randomUUID()}', 'WidgetShown', '{"app_id":"${appId}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
  ].join(';\n'));
}

/** Serve every preference read at `scale`, so the shell and the frames boot at it. */
async function bootAtScale(page: Page, scale: number): Promise<void> {
  await page.addInitScript((s) => {
    if (window.parent !== window) return;
    localStorage.setItem('lucidos-ui-scale', String(s));
  }, scale);
  await page.route('**/api/v1/preferences', async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    const resp = await route.fetch();
    const body = await resp.json() as Record<string, unknown>;
    return route.fulfill({ response: resp, json: { ...body, 'ui-scale': String(scale) } });
  });
}

const card = (page: Page): Locator => page.locator('.chat-exchange .inline-widget:visible').first();
const cardBox = (page: Page): Locator => card(page).locator('.widget-box');

function dashboardRoot(page: Page): Locator {
  return page.locator(isMobileViewport(page) ? '.dashboard-panel' : '.dashboard-rail');
}

const tile = (page: Page, appId: string): Locator => dashboardRoot(page).locator(`.dashboard-tile[data-dashboard-tile^="${appId}?"]`);

async function size(box: Locator): Promise<{ width: number; height: number }> {
  const b = await box.boundingBox();
  expect(b, 'box is laid out').not.toBeNull();
  return { width: b!.width, height: b!.height };
}

/** What the widget in `box` is told and draws, once it has been told `s`. The
 *  push and the report that follows it cross a frame boundary, so each is
 *  awaited. The box fades exactly when the widget's content overflows it. */
async function inside(box: Locator, s: Size) {
  const frame = box.frameLocator('iframe[data-role="app-ui-frame"]');
  await expect(frame.locator('#first')).toBeVisible({ timeout: 15_000 });
  await expect(frame.locator('html'), 'told its size').toHaveAttribute('data-widget-size', s);
  const overflows = await frame.locator('body').evaluate((body) => body.scrollHeight - innerHeight > 1);
  await expect(box.locator('.widget-box-fade'), 'fades exactly when it overflows').toHaveCount(overflows ? 1 : 0);
  return {
    told: await frame.locator('html').getAttribute('data-widget-size'),
    fontSize: await frame.locator('#first').evaluate((el) => getComputedStyle(el).fontSize),
    faded: overflows ? 1 : 0,
  };
}

/** Picks `s` in the card menu's Size row, then closes the menu. */
async function pickCardSize(page: Page, s: Size): Promise<void> {
  const menu = await openWidgetCardMenu(page, card(page));
  const option = menu.locator(`.widget-menu-size [role="radio"][aria-label="${s}"]`);
  await option.click();
  await expect(option).toHaveAttribute('aria-checked', 'true');
  await expect(cardBox(page)).toHaveAttribute('data-size', s);
  await page.keyboard.press('Escape');
  await expect(menu).toHaveCount(0);
}

async function setTileSize(page: Page, dashboard: 'desktop' | 'phone', appId: string, s: Size): Promise<void> {
  const resp = await apiRequest(page).post('/api/v1/widgets/arrange', {
    data: { dashboard, tiles: [{ app_id: appId, size: s }] },
  });
  expect(resp.ok(), `arrange: ${await resp.text()}`).toBeTruthy();
  await expect(tile(page, appId)).toHaveAttribute('data-size', s);
}

async function showDashboard(page: Page): Promise<void> {
  if (isMobileViewport(page)) {
    const toggle = page.locator('.mobile-thread-header .header-mark-group .dashboard-panel-toggle:visible');
    if (await toggle.getAttribute('aria-pressed') !== 'true') await toggle.click();
    await expect(dashboardRoot(page)).toHaveCSS('opacity', '1');
  }
  await waitForDashboardReady(dashboardRoot(page));
}

async function hideDashboard(page: Page): Promise<void> {
  if (!isMobileViewport(page)) return;
  const toggle = page.locator('.mobile-thread-header .header-mark-group .dashboard-panel-toggle:visible');
  if (await toggle.getAttribute('aria-pressed') === 'true') await toggle.click();
  await expect(card(page)).toBeVisible();
}

test.describe('One widget box', () => {
  let threadId: string;
  let appId: string;
  let dashboard: 'desktop' | 'phone';

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    written = [];
    threadId = randomUUID();
    appId = `e2e-box-${randomUUID().slice(0, 8)}`;
    dashboard = isMobileViewport(page) ? 'phone' : 'desktop';
  });

  test.afterEach(async ({ page }) => {
    await apiRequest(page).post('/api/v1/widgets/dashboard-remove', { data: { app_id: appId, dashboard } });
    psql(`DELETE FROM events WHERE thread_id = '${threadId}'; DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`);
    for (const path of written) await apiRequest(page).delete(`/api/v1/data/${path}`);
  });

  /** Invariants: an XL box is four cells wide at its true size in a card and a
   *  tile alike. A host narrower than that clamps the box and scrolls it
   *  sideways, never widening the page, and the widget is still told `xl`. */
  test('an XL widget keeps four cells inside a clamped card and tile, scrolling sideways', async ({ page }) => {
    await page.goto('/');
    await writeFile(page, `apps/${appId}/index.html`, OLD_CONTRACT_WIDGET);
    await writeFile(page, `apps/${appId}/manifest.json`, JSON.stringify({
      name: 'E2E run plan', description: 'This week of runs', kind: 'widget', reusable: true,
      sizes: ['medium', 'large', 'xl'],
    }));
    const added = await apiRequest(page).post('/api/v1/widgets/dashboard-add', { data: { app_id: appId, dashboard } });
    expect(added.ok(), `add: ${await added.text()}`).toBeTruthy();
    seedThreadShowingWidget(threadId, appId);

    await page.addInitScript((tid: string) => {
      if (window.parent !== window) return;
      localStorage.setItem('lucidos-focused-thread', tid);
    }, threadId);
    await navigateToApp(page);
    if (isMobileViewport(page)) await ensureOnThreadPane(page);
    await expect(card(page)).toBeVisible({ timeout: 15_000 });

    const scroller = (box: Locator) => box.locator(':scope > .widget-box-scroll');
    const canvas = (box: Locator) => scroller(box).locator('.widget-box-canvas');
    async function xlBox(box: Locator) {
      const frame = box.frameLocator('iframe[data-role="app-ui-frame"]');
      await expect(frame.locator('html'), 'told its size').toHaveAttribute('data-widget-size', 'xl', { timeout: 15_000 });
      const dims = await scroller(box).evaluate((el) => ({
        boxInnerWidth: el.clientWidth,
        scrolls: el.scrollWidth > el.clientWidth + 1,
        boxWidth: (el.parentElement as HTMLElement).getBoundingClientRect().width,
        viewportWidth: window.innerWidth,
      }));
      return {
        ...dims,
        canvasWidth: (await size(canvas(box))).width,
      };
    }

    const rem = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).fontSize));

    await pickCardSize(page, 'large');
    const large = await size(cardBox(page));
    await pickCardSize(page, 'xl');
    await card(page).scrollIntoViewIfNeeded();
    const inCard = await xlBox(cardBox(page));
    // Four cells: two large boxes and the gap between them, less the border.
    const gap = inCard.canvasWidth + 2 - 2 * large.width;
    expect(Math.abs(gap - 0.75 * rem), 'XL is two larges and a gap').toBeLessThanOrEqual(1.5);
    expect(inCard.boxWidth, 'the card never outgrows the viewport').toBeLessThanOrEqual(inCard.viewportWidth);

    await showDashboard(page);
    await setTileSize(page, dashboard, appId, 'xl');
    const inTile = await xlBox(tile(page, appId));
    await hideDashboard(page);
    expect(Math.abs(inTile.canvasWidth - inCard.canvasWidth), 'one true width').toBeLessThanOrEqual(1);
    expect(inTile.boxWidth, 'the tile never outgrows the viewport').toBeLessThanOrEqual(inTile.viewportWidth);

    for (const [place, seen] of [['card', inCard], ['tile', inTile]] as const) {
      const clamped = seen.boxInnerWidth < seen.canvasWidth - 1;
      expect(seen.scrolls, `${place} scrolls exactly when clamped`).toBe(clamped);
    }
    // A phone has two columns, so its tile is always clamped.
    if (isMobileViewport(page)) expect(inTile.scrolls, 'phone tile scrolls').toBe(true);
  });

  for (const scale of [100, 175]) {
    test(`the thread card draws the dashboard tile's box at every size, at ${scale}% UI scale`, async ({ page }) => {
      await page.goto('/');
      await writeFile(page, `apps/${appId}/index.html`, OLD_CONTRACT_WIDGET);
      await writeFile(page, `apps/${appId}/manifest.json`, JSON.stringify({
        name: 'E2E run plan', description: 'This week of runs', kind: 'widget', reusable: true,
      }));
      const added = await apiRequest(page).post('/api/v1/widgets/dashboard-add', { data: { app_id: appId, dashboard } });
      expect(added.ok(), `add: ${await added.text()}`).toBeTruthy();
      seedThreadShowingWidget(threadId, appId);

      await bootAtScale(page, scale);
      await page.addInitScript((tid: string) => {
        if (window.parent !== window) return;
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);
      if (isMobileViewport(page)) await ensureOnThreadPane(page);
      await expect(card(page)).toBeVisible({ timeout: 15_000 });

      for (const s of SIZES) {
        await pickCardSize(page, s);
        await card(page).scrollIntoViewIfNeeded();
        const inCard = { box: await size(cardBox(page)), ...await inside(cardBox(page), s) };

        await showDashboard(page);
        await setTileSize(page, dashboard, appId, s);
        const inTile = { box: await size(tile(page, appId)), ...await inside(tile(page, appId), s) };
        await hideDashboard(page);

        // I1: the same box, to 1 px.
        expect(Math.abs(inCard.box.width - inTile.box.width), `${s} width`).toBeLessThanOrEqual(1);
        expect(Math.abs(inCard.box.height - inTile.box.height), `${s} height`).toBeLessThanOrEqual(1);
        // I2: it clips in one place exactly when it clips in the other.
        expect(inCard.faded, `${s} fade`).toBe(inTile.faded);
        // I3: told the same size in both places.
        expect(inCard.told, `${s} card is told its size`).toBe(s);
        expect(inTile.told, `${s} tile is told its size`).toBe(s);
        // I5: no shrinking: the same text computes to the same px.
        expect(inCard.fontSize, `${s} font size`).toBe(inTile.fontSize);
      }
      // The old contract's three cards wrap in a small box, so it clips there.
      await pickCardSize(page, 'small');
      await expect(cardBox(page).locator('.widget-box-fade')).toHaveCount(1);
    });
  }
});
