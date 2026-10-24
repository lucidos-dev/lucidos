import { test, expect, Page, Locator } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, navigateToApp, rightClickInsideFrame } from './helpers';
import { ensureHomeThread, psql } from './db-helpers';
import { computeMinDashboardRailWidth } from '../src/store/paneMinimums';
import { DASHBOARD_CELL_REM, DASHBOARD_GAP_REM } from '../src/utils/dashboardGeometry';

/**
 * The desktop dashboard (ADRs 0432, 0496). Its widgets are tiles at their size
 * class, live, in a rail at the window's right edge, beside the header and the
 * panes. Each widget is a reusable folder written through the data API, added
 * to the dashboard over HTTP, so the spec needs no model.
 */

/** Every data file a test wrote, so `afterEach` removes them all. */
let written: string[] = [];
let added: string[] = [];
let seededThreads: string[] = [];

async function writeFile(page: Page, path: string, body: string): Promise<void> {
  written.push(path);
  const resp = await apiRequest(page).put(`/api/v1/data/${path}`, {
    headers: { 'Content-Type': 'text/plain' },
    data: body,
  });
  expect(resp.ok(), `PUT ${path}`).toBeTruthy();
}

/** A widget that shows a random number per load, so a reload is visible, and
 *  echoes the word of each `E2EDashboardPinged` event. */
function liveWidget(extraBody = ''): string {
  return '<!doctype html><html><head><script src="/api/v1/sdk.js"></script></head><body>'
    + '<p id="load"></p><p id="out">waiting</p>' + extraBody
    + '<script>document.getElementById("load").textContent = String(Math.random());'
    + 'lucidos.sse.on("E2EDashboardPinged", (d) => { document.getElementById("out").textContent = d.word; });'
    + 'lucidos.sse.connect();</script></body></html>';
}

async function writeWidget(page: Page, appId: string, manifest: Record<string, unknown>, html = liveWidget()): Promise<void> {
  await writeFile(page, `apps/${appId}/index.html`, html);
  await writeFile(page, `apps/${appId}/manifest.json`, JSON.stringify({
    name: appId, description: 'E2E dashboard tile', kind: 'widget', reusable: true, ...manifest,
  }));
}

async function addTile(page: Page, appId: string): Promise<void> {
  const resp = await apiRequest(page).post('/api/v1/widgets/dashboard-add', { data: { app_id: appId, dashboard: 'desktop' } });
  expect(resp.ok(), `add ${appId}: ${await resp.text()}`).toBeTruthy();
  added.push(appId);
}

/** A reusable widget, as a tile on the desktop dashboard. */
async function addToDashboard(page: Page, appId: string, manifest: Record<string, unknown>, html = liveWidget()): Promise<void> {
  await writeWidget(page, appId, manifest, html);
  await addTile(page, appId);
}

async function tileAppIds(page: Page, dashboard: 'phone' | 'desktop'): Promise<string[]> {
  const resp = await page.request.get(`/api/v1/dashboard?dashboard=${dashboard}`);
  expect(resp.ok(), await resp.text()).toBeTruthy();
  return ((await resp.json()) as { tiles: Array<{ app_id: string }> }).tiles.map((t) => t.app_id);
}

const rail = (page: Page): Locator => page.locator('.dashboard-rail');
/** The live grid. While the loading fade runs, a skeleton copy of the grid sits beside it. */
const grid = (page: Page): Locator => rail(page).locator('.dashboard-grid:not(.loading-fade-skeleton .dashboard-grid)');

async function openDashboard(page: Page): Promise<void> {
  await navigateToApp(page);
  await expect(rail(page).locator('.dashboard')).toBeVisible({ timeout: 15_000 });
  // The grid is inert until its tiles are seen, or the settle fuse runs out.
  await expect(grid(page).and(page.locator('.is-settling'))).toHaveCount(0);
}

const tile = (page: Page, appId: string): Locator => page.locator(`.dashboard-tile[data-dashboard-tile^="${appId}?"]`);
const frame = (page: Page, appId: string) => tile(page, appId).frameLocator('iframe');

async function box(locator: Locator) {
  const b = await locator.boundingBox();
  expect(b).not.toBeNull();
  return b!;
}

const rootFontPx = (page: Page) => page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).fontSize));

/** The rail's floor as the page computes it, with the gutter it publishes. */
async function railFloor(page: Page): Promise<number> {
  const gutter = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--scrollbar-gutter-width')) || 0);
  return computeMinDashboardRailWidth(await rootFontPx(page), gutter);
}

/** Drags the rail's divider until the rail is `width` wide. */
async function dragRailTo(page: Page, width: number): Promise<void> {
  const handle = await box(page.locator('.dashboard-rail-divider'));
  const y = handle.y + 200;
  await page.mouse.move(handle.x + handle.width / 2, y);
  await page.mouse.down();
  await page.mouse.move(handle.x - 10, y, { steps: 3 });
  await page.mouse.move(page.viewportSize()!.width - width, y, { steps: 8 });
  await page.mouse.up();
  await expect.poll(async () => Math.round((await box(rail(page))).width)).toBe(Math.round(width));
}

const columns = (page: Page) => grid(page)
  .evaluate((el) => getComputedStyle(el).getPropertyValue('--dashboard-columns').trim());

test.describe('dashboard on desktop', () => {
  let ids: { small: string; medium: string; large: string };

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // An earlier spec in the shard may have truncated the Home row.
    ensureHomeThread();
    written = [];
    added = [];
    seededThreads = [];
    const tag = randomUUID().slice(0, 8);
    ids = { small: `e2e-dash-s-${tag}`, medium: `e2e-dash-m-${tag}`, large: `e2e-dash-l-${tag}` };
    await page.goto('/');
  });

  test.afterEach(async ({ page }) => {
    for (const appId of added) {
      await apiRequest(page).post('/api/v1/widgets/dashboard-remove', { data: { app_id: appId, dashboard: 'desktop' } });
    }
    for (const path of written) await apiRequest(page).delete(`/api/v1/data/${path}`);
    for (const id of seededThreads) {
      psql(`DELETE FROM events WHERE thread_id = '${id}'; DELETE FROM thread_summaries WHERE thread_id = '${id}'`);
    }
  });

  test('a device shows the dashboard in its rail, with each widget a tile at its size', async ({ page }) => {
    await addToDashboard(page, ids.small, { sizes: ['small'] });
    await addToDashboard(page, ids.medium, {});
    await addToDashboard(page, ids.large, { sizes: ['large', 'small'] });
    await openDashboard(page);

    await expect(tile(page, ids.small)).toHaveAttribute('data-size', 'small');
    await expect(tile(page, ids.medium)).toHaveAttribute('data-size', 'medium');
    await expect(tile(page, ids.large)).toHaveAttribute('data-size', 'large');
    const gap = await grid(page).evaluate((el) => parseFloat(getComputedStyle(el).columnGap));
    const s = await box(tile(page, ids.small));
    const m = await box(tile(page, ids.medium));
    const l = await box(tile(page, ids.large));
    expect(Math.abs(s.width - s.height)).toBeLessThan(2);
    expect(Math.abs(m.width - (2 * s.width + gap))).toBeLessThan(2);
    expect(Math.abs(m.height - s.height)).toBeLessThan(2);
    expect(Math.abs(l.height - (2 * s.height + gap))).toBeLessThan(2);
    await expect(frame(page, ids.small).locator('html')).toHaveAttribute('data-widget-size', 'small');
    await expect(frame(page, ids.large).locator('html')).toHaveAttribute('data-widget-place', 'dashboard');
  });

  /** ADR 0532. A widget tile keeps its old cell box. An app shortcut is a
   *  quarter tile with no frame. A tap opens the app in the Canvas. */
  test('an app shortcut is a frameless quarter tile that opens its app, beside widgets at their old boxes', async ({ page }) => {
    const appId = `e2e-dash-app-${randomUUID().slice(0, 8)}`;
    await writeFile(page, `apps/${appId}/index.html`, '<!doctype html><html><body><p id="opened">opened</p></body></html>');
    await writeFile(page, `apps/${appId}/manifest.json`, JSON.stringify({ name: 'E2E Shortcut', description: 'E2E app shortcut' }));
    await addToDashboard(page, ids.medium, {});
    await addTile(page, appId);
    await openDashboard(page);

    const shortcut = tile(page, appId);
    await expect(shortcut).toHaveAttribute('data-kind', 'app');
    await expect(shortcut).toHaveAttribute('data-size', 'quarter');
    await expect(shortcut.locator('iframe')).toHaveCount(0);

    const rem = await rootFontPx(page);
    const cell = DASHBOARD_CELL_REM * rem;
    const gap = DASHBOARD_GAP_REM * rem;
    const m = await box(tile(page, ids.medium));
    expect(Math.abs(m.width - (2 * cell + gap))).toBeLessThan(1);
    expect(Math.abs(m.height - cell)).toBeLessThan(1);
    const q = await box(shortcut);
    expect(Math.abs(q.width - (cell - gap) / 2)).toBeLessThan(1);
    expect(Math.abs(q.height - (cell - gap) / 2)).toBeLessThan(1);

    await shortcut.getByRole('button', { name: /^Open E2E Shortcut/ }).click();
    await expect(page.locator('.app-ui-inline:visible')).toBeVisible({ timeout: 15_000 });
    await expect(rail(page)).toBeVisible();
  });

  /** Invariant: a tile's size comes from its class, never its content. */
  test('a widget taller than its tile is clipped under a fade and never scrolls inside', async ({ page }) => {
    await addToDashboard(page, ids.small, { sizes: ['small'] });
    await addToDashboard(page, ids.medium, {}, liveWidget('<div style="height: 3000px"></div>'));
    await openDashboard(page);

    const s = await box(tile(page, ids.small));
    const m = await box(tile(page, ids.medium));
    expect(Math.abs(m.height - s.height)).toBeLessThan(2);
    // The fade marks the clip. Opening it full size is the tile menu's job.
    await expect(tile(page, ids.medium).locator('.dashboard-tile-fade')).toBeVisible();
    await expect(tile(page, ids.medium).locator('.dashboard-tile-fade button')).toHaveCount(0);
    const html = frame(page, ids.medium).locator('html');
    expect(await html.evaluate((el) => getComputedStyle(el).overflow)).toBe('hidden');
    const t = await box(tile(page, ids.medium));
    await page.mouse.move(t.x + t.width / 2, t.y + t.height / 3);
    await page.mouse.wheel(0, 600);
    await page.waitForTimeout(300);
    expect(await html.evaluate((el) => el.ownerDocument.scrollingElement?.scrollTop ?? 0)).toBe(0);
  });

  /** Invariants: a tile added over HTTP appears without a reload, and only on
   *  the dashboard it was added to (ADR 0496). */
  test('a tile added to the desktop dashboard shows live, and the phone dashboard stays without it', async ({ page }) => {
    await writeWidget(page, ids.medium, {});
    await openDashboard(page);
    await expect(tile(page, ids.medium)).toHaveCount(0);
    await addTile(page, ids.medium);
    await expect(tile(page, ids.medium)).toBeVisible({ timeout: 15_000 });
    expect(await tileAppIds(page, 'desktop')).toContain(ids.medium);
    expect(await tileAppIds(page, 'phone')).not.toContain(ids.medium);
  });

  test('an emitted event updates a tile live', async ({ page }) => {
    await addToDashboard(page, ids.medium, {});
    await openDashboard(page);
    await expect(frame(page, ids.medium).locator('#out')).toHaveText('waiting');

    const word = `ping-${randomUUID().slice(0, 6)}`;
    const resp = await apiRequest(page).post('/api/v1/events/emit', {
      data: { event_type: 'E2EDashboardPinged', payload: { word, summary: 'E2E dashboard ping' }, transient: true },
    });
    expect(resp.ok(), await resp.text()).toBeTruthy();
    await expect(frame(page, ids.medium).locator('#out')).toHaveText(word);
  });

  /** Invariants I1 and I8: a right-click inside a live tile's frame opens the
   *  tile menu at the pointer, and leaves edit mode to that menu. */
  test('a right-click inside a tile opens the tile menu at the pointer, not edit mode', async ({ page }) => {
    await addToDashboard(page, ids.medium, {});
    await openDashboard(page);
    await expect(frame(page, ids.medium).locator('#out')).toHaveText('waiting');

    const menu = await rightClickInsideFrame(page, tile(page, ids.medium).locator('iframe'));
    await expect(page.locator('.dashboard-tile-shield')).toHaveCount(0);
    await menu.getByRole('menuitem', { name: 'Edit dashboard' }).click();
    await expect(page.locator('.dashboard-tile-shield')).toHaveCount(1);
    await page.locator('.dashboard-done').click();
  });

  /** Invariant: the rail is outside the header and the panes. */
  test('the rail runs the full height at the right edge, and the header and panes end where it begins', async ({ page }) => {
    await openDashboard(page);
    const viewport = page.viewportSize()!;
    const r = await box(rail(page));
    expect(Math.abs(r.x + r.width - viewport.width)).toBeLessThan(1);
    expect(r.y).toBe(0);
    expect(Math.abs(r.height - viewport.height)).toBeLessThan(1);
    for (const selector of ['.app-header', '.split-layout']) {
      const b = await box(page.locator(selector).first());
      expect(b.x + b.width, `${selector} ends at the rail`).toBeLessThanOrEqual(r.x + 1);
    }
  });

  /** Invariant: the menu drawer emerges from the split divider, whatever room the rail takes. */
  test('the menu drawer opens at the Canvas pane\'s leading edge while the rail shows', async ({ page }) => {
    await openDashboard(page);
    const row = page.locator('.app-header .content-header-elements');
    await row.locator('.hamburger-panel').click();
    const backdrop = page.locator('.drawer-backdrop');
    await expect(page.locator('.drawer')).toBeVisible();
    expect(Math.abs((await box(backdrop)).x - (await box(row)).x)).toBeLessThan(1);
  });

  /** Invariant: an open overlay inerts the rail, as it does the rest of the shell. */
  test('an open overlay makes the rail inert like the rest of the shell', async ({ page }) => {
    await openDashboard(page);
    const head = rail(page).locator('.dashboard-head');
    const pe = () => head.evaluate((el) => getComputedStyle(el).pointerEvents);
    expect(await pe()).not.toBe('none');
    await page.locator('.compose-destination-picker .dropdown-trigger:visible').first().click();
    await expect(page.locator('.dropdown-menu:visible').first()).toBeVisible();
    await expect.poll(pe).toBe('none');
    await page.keyboard.press('Escape');
    await expect.poll(pe).not.toBe('none');
  });

  /** Invariants: a drag only clamps, at two dashboard cells, and the width persists.
   *  Resizing never moves a frame, so no tile reloads. */
  test('a drag on the rail stops at two dashboard cells, reloads no tile, and survives a reload', async ({ page }) => {
    await addToDashboard(page, ids.small, { sizes: ['small'] });
    await openDashboard(page);
    const loaded = await frame(page, ids.small).locator('#load').textContent();
    const floor = await railFloor(page);
    const handle = await box(page.locator('.dashboard-rail-divider'));
    await page.mouse.move(handle.x + handle.width / 2, handle.y + 200);
    await page.mouse.down();
    await page.mouse.move(handle.x + 40, handle.y + 200, { steps: 4 });
    await page.mouse.move(page.viewportSize()!.width - 5, handle.y + 200, { steps: 8 });
    await page.mouse.up();
    await expect.poll(async () => Math.round((await box(rail(page))).width)).toBe(floor);
    await expect(grid(page)).toHaveCSS('--dashboard-columns', '2');
    // The floor holds two whole cells, so they draw at their own edge.
    const cell = DASHBOARD_CELL_REM * await rootFontPx(page);
    expect(Math.abs((await box(tile(page, ids.small))).width - cell)).toBeLessThan(1);
    await expect(frame(page, ids.small).locator('#load')).toHaveText(loaded ?? '');

    await page.reload();
    await expect(rail(page).locator('.dashboard')).toBeVisible({ timeout: 15_000 });
    expect(Math.round((await box(rail(page))).width)).toBe(floor);
  });

  /** Invariant: the Home icon opens the chat and never the rail, even in Home. The rail toggle, in the window's corner in both states, shows and hides the rail. */
  test('the Home icon opens only the chat, and the rail toggle hides and shows the rail', async ({ page }) => {
    await addToDashboard(page, ids.small, { sizes: ['small'] });
    await openDashboard(page);
    const loaded = await frame(page, ids.small).locator('#load').textContent();
    await page.locator('.desktop-header .home-thread-btn').click();
    await expect(page.locator('.thread-title-menu:visible')).toHaveText('Home');
    await expect(rail(page)).toBeVisible();

    const button = page.locator('.dashboard-rail-toggle');
    const bell = page.locator('.desktop-header .notifications-bell');
    // Shown, it ends the rail's head, past Edit.
    const shown = await box(button);
    expect(shown.x).toBeGreaterThan((await box(rail(page))).x);
    const edit = await box(rail(page).locator('.dashboard-edit'));
    expect(shown.x).toBeGreaterThan(edit.x + edit.width);

    await button.click();
    await expect(rail(page)).toBeHidden();
    await expect(button).toHaveAttribute('aria-pressed', 'false');
    // The shell's padding slides back, so the header reaches the edge once it lands.
    await expect.poll(async () => {
      const header = await box(page.locator('.app-header').first());
      return Math.abs(header.x + header.width - page.viewportSize()!.width);
    }).toBeLessThan(1);
    // Hidden, it rests right of the bell, one gap past it.
    await expect.poll(async () => (await box(button)).x - ((await box(bell)).x + (await box(bell)).width)).toBeGreaterThan(0);
    // Shown again soon, the tile is the one that was loaded (ADR 0422).
    await button.click();
    await expect(rail(page)).toBeVisible();
    await expect(frame(page, ids.small).locator('#load')).toHaveText(loaded ?? '');
    await button.click();
    await expect(rail(page)).toBeHidden();
    // In Home already, the Home icon still leaves the rail to its toggle.
    await page.locator('.desktop-header .home-thread-btn').click();
    await expect(button).toHaveAttribute('aria-pressed', 'false');
    await expect(rail(page)).toBeHidden();

    await page.reload();
    await expect(page.locator('.dashboard-rail-toggle')).toHaveAttribute('aria-pressed', 'false', { timeout: 15_000 });
    await expect(rail(page)).toBeHidden();

    await page.locator('.dashboard-rail-toggle').click();
    await expect(rail(page)).toBeVisible();
  });

  test('an app opened from a tile fills the Canvas while the rail keeps its tile loaded', async ({ page }) => {
    await addToDashboard(page, ids.medium, {});
    await openDashboard(page);
    const loaded = await frame(page, ids.medium).locator('#load').textContent();

    const menu = await rightClickInsideFrame(page, tile(page, ids.medium).locator('iframe'));
    await menu.getByRole('menuitem', { name: /Open in Canvas/ }).click();
    await expect(page.locator('.app-ui-inline:visible')).toBeVisible();
    await expect(rail(page)).toBeVisible();
    await expect(frame(page, ids.medium).locator('#load')).toHaveText(loaded ?? '');
  });

  /** Invariant: the rail alone sizes the dashboard, whatever the Conversation shows. */
  test('opening a long thread on the left leaves the dashboard width and columns alone', async ({ page }) => {
    await addToDashboard(page, ids.medium, {});
    const threadId = randomUUID();
    seededThreads.push(threadId);
    const now = new Date().toISOString();
    const long = 'A long line of text for the transcript to wrap. '.repeat(400);
    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) `
        + `VALUES ('${threadId}', 'E2E long thread', 'chat', '${now}', 1, false, false, 'idle', 'inbox', false, 0, 'none', false)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
        + `VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"${long}","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));
    await openDashboard(page);

    const dashboard = rail(page).locator('.dashboard');
    const before = { width: (await box(dashboard)).width, columns: await columns(page) };
    expect(before.columns).toMatch(/^\d+$/);

    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, threadId);
    await expect(page.locator('.chat-exchange').first()).toBeVisible({ timeout: 15_000 });
    expect((await box(dashboard)).width).toBe(before.width);
    expect(await columns(page)).toBe(before.columns);
  });

  /** Invariants: a tile keeps its size at every dashboard width, and the spare
   *  width splits evenly, so the grid is centred. No tile reloads. */
  test('a tile keeps its size as the rail crosses a column boundary, centred in the dashboard', async ({ page }) => {
    await page.setViewportSize({ width: 1600, height: 900 });
    await addToDashboard(page, ids.medium, {});
    await openDashboard(page);
    const loaded = await frame(page, ids.medium).locator('#load').textContent();
    const rem = await rootFontPx(page);
    const floor = await railFloor(page);
    const step = (DASHBOARD_CELL_REM + DASHBOARD_GAP_REM) * rem;
    const spare = 80;

    await dragRailTo(page, floor + spare);
    expect(await columns(page)).toBe('2');
    const two = await box(tile(page, ids.medium));
    const area = await box(grid(page));
    // The medium tile spans both columns, so its insets are the grid's.
    const left = two.x - area.x;
    const right = area.x + area.width - (two.x + two.width);
    expect(Math.abs(left - right)).toBeLessThanOrEqual(1);
    expect(Math.abs(left - spare / 2)).toBeLessThanOrEqual(1);

    await dragRailTo(page, floor + step + spare / 2);
    expect(await columns(page)).toBe('3');
    const three = await box(tile(page, ids.medium));
    expect(Math.abs(three.width - two.width)).toBeLessThan(1);
    expect(Math.abs(three.height - two.height)).toBeLessThan(1);
    await expect(frame(page, ids.medium).locator('#load')).toHaveText(loaded ?? '');
  });

  test.describe('edit mode', () => {
    async function dragOnto(page: Page, from: string, to: string): Promise<void> {
      const a = await box(tile(page, from));
      const b = await box(tile(page, to));
      await page.mouse.move(a.x + a.width / 2, a.y + a.height / 2);
      await page.mouse.down();
      await page.mouse.move(a.x + a.width / 2 + 10, a.y + a.height / 2, { steps: 3 });
      await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 12 });
      await page.mouse.up();
    }

    const order = (page: Page, appId: string) => tile(page, appId).evaluate((el) => (el as HTMLElement).style.order);

    /** Invariant: reordering never moves an iframe in the DOM. */
    test('a drag reorders without reloading a frame, survives a reload, and reaches a second browser', async ({ page, browser }) => {
      await addToDashboard(page, ids.small, { sizes: ['small'] });
      await addToDashboard(page, ids.medium, {});
      await addToDashboard(page, ids.large, { sizes: ['large'] });
      await openDashboard(page);
      const loads = Object.fromEntries(await Promise.all(Object.values(ids).map(async (id) => [id, await frame(page, id).locator('#load').textContent()])));
      const before = [await order(page, ids.small), await order(page, ids.large)];

      await page.locator('.dashboard-edit').click();
      await expect(page.locator('.dashboard-tile-shield')).toHaveCount(3);
      await dragOnto(page, ids.small, ids.large);
      await expect.poll(() => order(page, ids.small)).toBe(before[1]);
      for (const id of Object.values(ids)) await expect(frame(page, id).locator('#load')).toHaveText(loads[id]!);

      await page.locator('.dashboard-done').click();
      await expect(page.locator('.dashboard-tile-shield')).toHaveCount(0);

      await page.reload();
      await expect(tile(page, ids.small)).toBeVisible({ timeout: 15_000 });
      expect(await order(page, ids.small)).toBe(before[1]);

      const other = await browser.newContext();
      const second = await other.newPage();
      await navigateToApp(second);
      await expect(tile(second, ids.small)).toBeVisible({ timeout: 15_000 });
      expect(await order(second, ids.small)).toBe(before[1]);
      await other.close();
    });

    /** Invariant: a widget that declares no sizes takes all three, and a size
     *  change reloads no frame. */
    test('a widget that declares no sizes resizes through the pill without reloading', async ({ page }) => {
      await addToDashboard(page, ids.medium, {});
      await openDashboard(page);
      await expect(tile(page, ids.medium)).toHaveAttribute('data-size', 'medium');
      const loaded = await frame(page, ids.medium).locator('#load').textContent();

      await page.locator('.dashboard-edit').click();
      const pill = tile(page, ids.medium).locator('.dashboard-size-pill button');
      await expect(pill).toHaveText(['S', 'M', 'L']);
      await tile(page, ids.medium).locator('.dashboard-size-pill button[aria-label="large"]').click();
      await expect(tile(page, ids.medium)).toHaveAttribute('data-size', 'large');
      await expect(frame(page, ids.medium).locator('html')).toHaveAttribute('data-widget-size', 'large');
      await expect(frame(page, ids.medium).locator('#load')).toHaveText(loaded ?? '');
      await page.locator('.dashboard-done').click();
    });

    test('Escape leaves edit mode, and so does Done', async ({ page }) => {
      await addToDashboard(page, ids.medium, {});
      await openDashboard(page);
      await page.locator('.dashboard-edit').click();
      await expect(page.locator('.dashboard-tile-shield')).toHaveCount(1);
      await page.keyboard.press('Escape');
      await expect(page.locator('.dashboard-tile-shield')).toHaveCount(0);
      await page.locator('.dashboard-edit').click();
      await page.locator('.dashboard-done').click();
      await expect(page.locator('.dashboard-tile-shield')).toHaveCount(0);
    });
  });
});
