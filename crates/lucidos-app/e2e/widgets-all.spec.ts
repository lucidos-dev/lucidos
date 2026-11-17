import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { readFileSync } from 'node:fs';
import { apiRequest, assertHealthy, navigateToApp, isMobileViewport, hideDashboardRail, openWidgetCardMenu, rightClickInsideFrame } from './helpers';
import { psql } from './db-helpers';

/**
 * Widgets (ADRs 0402, 0466) at desktop and phone width. The turn that showed a
 * widget holds a card with the widget inline, which survives a reload. A
 * legacy pin draws no chip, since the shelf is gone.
 *
 * The thread, its `WidgetShown` and any legacy `WidgetPinned` are seeded
 * directly, so the spec needs no model. The widget's folder goes through the data API,
 * which commits it.
 */

const WIDGET_NAME = 'E2E fare grid';
const THREAD_TITLE = 'E2E widgets for a trip to Stockholm';

/** Every data file a test wrote, so `afterEach` removes them all. */
let written: string[] = [];

async function writeFile(page: Page, path: string, body: string): Promise<void> {
  written.push(path);
  const resp = await apiRequest(page).put(`/api/v1/data/${path}`, {
    headers: { 'Content-Type': 'text/plain' },
    data: body,
  });
  expect(resp.ok(), `PUT ${path}`).toBeTruthy();
}

/** A widget a few lines tall, well inside the inline widget's clip. */
const SHORT_WIDGET = '<!doctype html><html><head><script src="/api/v1/sdk.js"></script></head><body><h1>Fares</h1></body></html>';
/** A widget far taller than any widget box, so its inline widget clips. */
const TALL_WIDGET = '<!doctype html><html><head><script src="/api/v1/sdk.js"></script></head>'
  + '<body><h1>Fares</h1><div style="height: 3000px"></div></body></html>';

async function writeWidget(page: Page, appId: string, threadId: string, html: string): Promise<void> {
  await writeFile(page, `apps/${appId}/index.html`, html);
  await writeFile(page, `apps/${appId}/manifest.json`, JSON.stringify({
    name: WIDGET_NAME,
    description: 'Fares by date',
    kind: 'widget',
    origin_thread_id: threadId,
  }));
}

function seedThreadShowingWidget(threadId: string, appId: string, { legacyPin = false, params = {} }: { legacyPin?: boolean; params?: Record<string, string> } = {}): void {
  const now = new Date().toISOString();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) `
      + `VALUES ('${threadId}', '${THREAD_TITLE}', 'chat', '${now}', 2, false, true, 'idle', 'inbox', false, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
      + `VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"compare the fares","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
      + `VALUES ('${randomUUID()}', 'WidgetShown', '{"app_id":"${appId}","params":${JSON.stringify(params)}}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    // Nothing writes a pin any more, but old rows remain.
    ...(legacyPin ? [
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
        + `VALUES ('${randomUUID()}', 'WidgetPinned', '{"app_id":"${appId}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ] : []),
  ].join(';\n'));
}

async function openThread(page: Page, threadId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  await navigateToApp(page);
}

const card = (page: Page) => page.locator('.chat-exchange .inline-widget:visible').first();

test.describe('Widgets', () => {
  let threadId: string;
  let appId: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    written = [];
    threadId = randomUUID();
    appId = `e2e-widget-${randomUUID().slice(0, 8)}`;
    await hideDashboardRail(page);
    await page.goto('/');
  });

  test.afterEach(async ({ page }) => {
    psql(`DELETE FROM events WHERE thread_id = '${threadId}'; DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`);
    for (const path of written) {
      await apiRequest(page).delete(`/api/v1/data/${path}`);
    }
  });

  test('a refused video poster stays hidden while play remains available', async ({ page }) => {
    seedThreadShowingWidget(threadId, 'lucidos-video-player', { params: { clip: `artifacts/missing-${randomUUID()}.mp4` } });
    await openThread(page, threadId);
    const frame = card(page).frameLocator('iframe');
    await expect(frame.locator('#stage')).toHaveAttribute('title', 'Preview unavailable. Tap to play.');
    expect(await frame.locator('#poster').evaluate(image => getComputedStyle(image).display)).toBe('none');
    await expect(frame.locator('#play')).toBeVisible();
    await expect(frame.locator('video')).not.toHaveAttribute('src');
  });

  test('a built-in video draws a real poster with no idle media and loads on play', async ({ page }) => {
    const path = `artifacts/e2e-poster-${randomUUID()}.mp4`;
    written.push(path);
    const response = await apiRequest(page).put(`/api/v1/data/${path}`, {
      headers: { 'Content-Type': 'video/mp4' },
      data: readFileSync(new URL('./__fixtures__/poster-clip.mp4', import.meta.url)),
    });
    expect(response.ok()).toBeTruthy();
    seedThreadShowingWidget(threadId, 'lucidos-video-player', { params: { clip: path } });
    await openThread(page, threadId);
    const frame = card(page).frameLocator('iframe');
    const poster = frame.locator('#poster');
    const video = frame.locator('video');
    await expect(poster).toHaveAttribute('src', /^data:image\/jpeg;base64,/, { timeout: 30_000 });
    await expect.poll(() => poster.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBeGreaterThan(0);
    await expect(video).not.toHaveAttribute('src');
    expect(await video.evaluate((media: HTMLVideoElement) => media.readyState)).toBe(0);
    await expect(frame.locator('#time')).toHaveText('0:00 / 0:02');
    await frame.locator('#play').click();
    await expect(video).toHaveAttribute('src', /#t=0.001$/);
    await expect.poll(() => video.evaluate((media: HTMLVideoElement) => media.currentTime)).toBeGreaterThan(0);
    await frame.locator('#play').click();
    await page.reload();
    await expect(card(page).frameLocator('iframe').locator('#poster'))
      .toHaveAttribute('src', /^data:image\/jpeg;base64,/, { timeout: 30_000 });
    await expect(card(page).frameLocator('iframe').locator('video')).not.toHaveAttribute('src');
  });

  /** I8 and I9: a legacy pin still loads, draws no chip, and its card stays. */
  test('shows the widget inline in its turn, with no chip for a legacy pin, and survives a reload', async ({ page }) => {
    await writeWidget(page, appId, threadId, SHORT_WIDGET);
    seedThreadShowingWidget(threadId, appId, { legacyPin: true });
    await openThread(page, threadId);

    await expect(card(page)).toHaveAttribute('aria-label', WIDGET_NAME, { timeout: 15_000 });
    await expect(card(page).locator('iframe[data-role="app-ui-frame"]')).toHaveAttribute(
      'src', new RegExp(`/app/${appId}/`),
    );
    // It fits its box, so the card does not clip it.
    await expect(card(page).locator('.widget-box-fade')).toHaveCount(0);
    await expect(page.locator('.widget-chip, .widget-shelf')).toHaveCount(0);

    // It draws no bar, and its menu offers no pin.
    await expect(card(page).locator('.widget-bar')).toHaveCount(0);
    await openWidgetCardMenu(page, card(page));
    await expect(page.getByRole('menuitem', { name: 'Open in Canvas' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: /pin|shelf/i })).toHaveCount(0);
    await page.keyboard.press('Escape');

    // The apps list never holds it.
    const apps = await (await page.request.get('/api/v1/apps')).json() as Array<{ id: string }>;
    expect(apps.some((a) => a.id === appId)).toBe(false);

    await page.reload();
    await expect(card(page)).toBeVisible({ timeout: 15_000 });
  });

  /** Invariant I1: the frame forwards an unclaimed right-click to the card. */
  test('a right-click inside the widget opens its menu at the pointer', async ({ page }) => {
    test.skip(isMobileViewport(page), 'a phone places the menu against the card, not the pointer');
    await writeWidget(page, appId, threadId, SHORT_WIDGET);
    seedThreadShowingWidget(threadId, appId);
    await openThread(page, threadId);
    await expect(card(page)).toHaveAttribute('aria-label', WIDGET_NAME, { timeout: 15_000 });
    await expect(card(page).frameLocator('iframe').locator('html')).toHaveAttribute('data-widget-place', 'inline');

    const menu = await rightClickInsideFrame(page, card(page).locator('.widget-frame'));
    await expect(menu.getByRole('menuitem', { name: 'Open in Canvas' })).toBeVisible();
  });

  test("a tall widget is clipped to its box under a fade, and its menu's Open in Canvas shows it whole", async ({ page }) => {
    await writeWidget(page, appId, threadId, TALL_WIDGET);
    seedThreadShowingWidget(threadId, appId);
    await openThread(page, threadId);

    await expect(card(page).locator('.widget-box-fade[data-edge="bottom"]')).toBeVisible({ timeout: 15_000 });
    // The box sets the frame, so the frame never grows to the content.
    const [frameBox, boxBox] = await Promise.all([
      card(page).locator('.widget-frame').boundingBox(),
      card(page).locator('.widget-box').boundingBox(),
    ]);
    expect(frameBox && boxBox).toBeTruthy();
    expect(frameBox!.height).toBeLessThanOrEqual(boxBox!.height);

    await openWidgetCardMenu(page, card(page));
    await page.getByRole('menuitem', { name: 'Open in Canvas' }).click();
    await expect(page.locator(`.app-ui-inline iframe[data-role="app-ui-frame"][src*="${appId}"]:visible`))
      .toHaveCount(1, { timeout: 15_000 });
  });

  /** The card is a bare box like a dashboard tile, centred in its turn's
   *  content column: its parent's content box. */
  test('centres the card in its turn, with no bar and no shadow', async ({ page }) => {
    await writeWidget(page, appId, threadId, SHORT_WIDGET);
    seedThreadShowingWidget(threadId, appId);
    await openThread(page, threadId);
    await expect(card(page)).toHaveAttribute('aria-label', WIDGET_NAME, { timeout: 15_000 });
    const geo = await card(page).evaluate((el) => {
      const r = el.getBoundingClientRect();
      const parent = el.parentElement!.getBoundingClientRect();
      const pad = getComputedStyle(el.parentElement!);
      return {
        left: r.left - parent.left - parseFloat(pad.paddingLeft),
        right: parent.right - parseFloat(pad.paddingRight) - r.right,
        shadow: getComputedStyle(el).boxShadow,
      };
    });
    expect(Math.abs(geo.left - geo.right)).toBeLessThanOrEqual(1);
    expect(geo.shadow).toBe('none');
  });
});
