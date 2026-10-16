import { test, expect, Page, Locator } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, ensureOnThreadPane, gotoWithRetry, touchPress, waitForVisibleInput, waitForWorkspaceReady } from './helpers';
import { homeThreadIdInDb, psql } from './db-helpers';

/**
 * Home on a phone (ADR 0432): one pane with two faces. The board face shows the
 * widgets pinned to Home with the prompt docked under it, reading "Message
 * Home". A tap in the prompt slides the Home chat up, and the Home icon slides
 * it back down, with no tile reloaded. The phone keeps its own arrangement,
 * which copies desktop until a phone arranges it.
 */

let written: string[] = [];
let pinned: string[] = [];

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

async function pinToHome(page: Page, appId: string, manifest: Record<string, unknown> = {}): Promise<void> {
  await writeFile(page, `apps/${appId}/index.html`, LIVE_WIDGET);
  await writeFile(page, `apps/${appId}/manifest.json`, JSON.stringify({
    name: appId, description: 'E2E board tile', kind: 'widget', reusable: true, sizes: ['medium', 'small'], ...manifest,
  }));
  const resp = await apiRequest(page).post('/api/v1/widgets/pin', { data: { app_id: appId, thread_id: homeThreadIdInDb() } });
  expect(resp.ok(), `pin ${appId}: ${await resp.text()}`).toBeTruthy();
  pinned.push(appId);
}

async function arrange(page: Page, layout: 'phone' | 'desktop', tiles: Array<Record<string, unknown>>): Promise<void> {
  const resp = await apiRequest(page).post('/api/v1/widgets/arrange', { data: { layout, tiles } });
  expect(resp.ok(), `arrange ${layout}: ${await resp.text()}`).toBeTruthy();
}

async function boardTiles(page: Page, layout: 'phone' | 'desktop'): Promise<Array<{ app_id: string; size: string; hidden?: boolean }>> {
  const resp = await page.request.get(`/api/v1/home-board?layout=${layout}`);
  return (await resp.json()).tiles;
}

/** A cold start with no stored focus lands on Home, on its board face. */
async function openHome(page: Page): Promise<void> {
  await page.addInitScript(() => localStorage.removeItem('lucidos-focused-thread'));
  await gotoWithRetry(page, '/');
  await ensureOnThreadPane(page);
  await waitForVisibleInput(page);
  await waitForWorkspaceReady(page);
  await expect(page.locator('.home-pane:visible')).toBeVisible({ timeout: 15_000 });
}

const pane = (page: Page) => page.locator('.home-pane:visible');
const tile = (page: Page, appId: string): Locator => pane(page).locator(`.home-board-tile[data-home-tile^="${appId}?"]`);
const frame = (page: Page, appId: string) => tile(page, appId).frameLocator('iframe');
const order = (page: Page, appId: string) => tile(page, appId).evaluate((el) => Number((el as HTMLElement).style.order));

test.describe('Home on a phone', () => {
  let a: string;
  let b: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    written = [];
    pinned = [];
    const tag = randomUUID().slice(0, 8);
    a = `e2e-hp-a-${tag}`;
    b = `e2e-hp-b-${tag}`;
    await page.goto('/');
  });

  test.afterEach(async ({ page }) => {
    const home = homeThreadIdInDb();
    for (const appId of pinned) {
      await apiRequest(page).post('/api/v1/widgets/unpin', { data: { app_id: appId, thread_id: home } });
    }
    for (const path of written) await apiRequest(page).delete(`/api/v1/data/${path}`);
  });

  test('a link into Home lands on its chat face, where a reply is', async ({ page }) => {
    await gotoWithRetry(page, `/#thread=${homeThreadIdInDb()}`);
    await ensureOnThreadPane(page);
    await expect(pane(page)).toHaveAttribute('data-home-face', 'chat', { timeout: 15_000 });
  });

  test('a cold start with no stored focus shows the board face, with the new-thread prompt under it', async ({ page }) => {
    await pinToHome(page, a);
    await openHome(page);
    await expect(pane(page)).toHaveAttribute('data-home-face', 'board');
    await expect(tile(page, a)).toBeVisible();
    await expect(page.locator('[data-role="prompt-input"]:visible')).toHaveAttribute('placeholder', 'What can I help with?');
  });

  test('sending from the board face lands in the Home thread and shows the chat face', async ({ page }) => {
    await openHome(page);
    const text = `Hello Home ${randomUUID().slice(0, 6)}`;
    const input = await waitForVisibleInput(page);
    await input.click();
    await expect(pane(page)).toHaveAttribute('data-home-face', 'chat');
    await input.fill(text);
    // A phone's Enter adds a line; Send sends.
    await page.locator('button[aria-label="Send message"]:visible').click();
    await expect(pane(page).locator('.home-pane-chat .chat-exchange', { hasText: text })).toBeVisible({ timeout: 15_000 });
    const home = homeThreadIdInDb();
    await expect.poll(() => psql(
      `SELECT count(*) FROM events WHERE thread_id = '${home}' AND event_type = 'MessageReceived' AND payload->>'text' = '${text}'`,
    ).trim()).toBe('1');
  });

  /** Invariant: switching faces never reloads a tile within the off-screen hold. */
  test('a tap on the Home icon slides the chat down to the board, and no tile reloaded', async ({ page }) => {
    await pinToHome(page, a);
    await openHome(page);
    const loaded = await frame(page, a).locator('#load').textContent();

    await (await waitForVisibleInput(page)).click();
    await expect(pane(page)).toHaveAttribute('data-home-face', 'chat');
    // An open keyboard takes the header out of reach, so put it away first.
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await expect(page.locator('html[data-keyboard-active]')).toHaveCount(0);
    await page.locator('.home-thread-btn:visible').first().click();
    await expect(pane(page)).toHaveAttribute('data-home-face', 'board');
    await expect(frame(page, a).locator('#load')).toHaveText(loaded ?? '');
  });

  test('the phone starts as a copy of desktop, and its own arrangement leaves desktop alone', async ({ page }) => {
    await pinToHome(page, a);
    await pinToHome(page, b);
    await arrange(page, 'desktop', [{ app_id: b, size: 'small' }, { app_id: a, size: 'medium' }]);
    const desktopBefore = await boardTiles(page, 'desktop');
    await openHome(page);

    expect(await order(page, b)).toBeLessThan(await order(page, a));
    await expect(tile(page, b)).toHaveAttribute('data-size', 'small');

    await pane(page).locator('.home-board-edit').click();
    await tile(page, a).locator('.home-board-size-pill button[aria-label="small"]').click();
    await expect.poll(async () => (await boardTiles(page, 'phone')).find((t) => t.app_id === a)?.size).toBe('small');
    expect(await boardTiles(page, 'desktop')).toEqual(desktopBefore);
  });

  /** Invariant: a phone narrower than two cells shrinks them to fit two
   *  columns, and never overflows the board. */
  test('a medium tile fills the two columns inside the board, with no overflow', async ({ page }) => {
    await pinToHome(page, a);
    await openHome(page);
    const board = pane(page).locator('.home-board');
    await expect(board).toHaveCSS('--home-board-columns', '2');
    const content = await board.evaluate((el) => {
      const r = el.getBoundingClientRect();
      const s = getComputedStyle(el);
      return { left: r.left + parseFloat(s.paddingLeft), right: r.right - parseFloat(s.paddingRight) };
    });
    const t = (await tile(page, a).boundingBox())!;
    expect(t.x).toBeGreaterThanOrEqual(content.left - 1);
    expect(t.x + t.width).toBeLessThanOrEqual(content.right + 1);
    expect(Math.abs(t.x - content.left - (content.right - t.x - t.width))).toBeLessThanOrEqual(1);
    expect(await board.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
  });

  test('a tile hidden on the phone mounts no frame there', async ({ page }) => {
    await pinToHome(page, a);
    await pinToHome(page, b);
    await arrange(page, 'phone', [{ app_id: a, size: 'medium' }, { app_id: b, size: 'medium', hidden: true }]);
    await openHome(page);
    await expect(tile(page, a)).toBeVisible();
    await expect(tile(page, b)).toHaveCount(0);
    await expect(pane(page).locator(`iframe[src*="/app/${b}/"]`)).toHaveCount(0);
  });

  test('a finger drag over a live tile moves nothing, and a hold enters edit mode', async ({ page, browserName }) => {
    test.skip(browserName === 'webkit', 'CDP touch input is Chromium only');
    await pinToHome(page, a);
    await pinToHome(page, b);
    await openHome(page);
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
    await expect(pane(page).locator('.home-board-tile-shield')).toHaveCount(0);
    expect([await order(page, a), await order(page, b)]).toEqual(before);

    const cdp = await page.context().newCDPSession(page);
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [from] });
    await page.waitForTimeout(800);
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    await expect(pane(page).locator('.home-board-tile-shield')).toHaveCount(2);
  });
});
