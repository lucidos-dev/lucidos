import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, navigateToApp, clickVisibleElement, isMobileViewport } from './helpers';
import { psql } from './db-helpers';

/**
 * Widgets (ADRs 0402, 0407) at desktop and phone width. The turn that showed a
 * widget holds a card with the widget inline. A pinned widget also has a chip
 * on the title row, which drops it open under the title. Both survive a reload.
 *
 * The thread, its `WidgetShown` and any `WidgetPinned` are seeded directly, so
 * the spec needs no model. The widget's folder goes through the data API,
 * which commits it.
 */

const WIDGET_NAME = 'E2E fare grid';
/** Long enough that a chip beside it would squeeze it on a phone. */
const THREAD_TITLE = 'E2E widgets for a trip to Stockholm';

async function writeFile(page: Page, path: string, body: string): Promise<void> {
  const resp = await apiRequest(page).put(`/api/v1/data/${path}`, {
    headers: { 'Content-Type': 'text/plain' },
    data: body,
  });
  expect(resp.ok(), `PUT ${path}`).toBeTruthy();
}

/** A widget a few lines tall, well inside the card's clip. */
const SHORT_WIDGET = '<!doctype html><html><head><script src="/api/v1/sdk.js"></script></head><body><h1>Fares</h1></body></html>';
/** A widget far taller than one phone screen, so its card clips. */
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

function seedThreadShowingWidget(threadId: string, appId: string, { pinned }: { pinned: boolean }): void {
  const now = new Date().toISOString();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) `
      + `VALUES ('${threadId}', '${THREAD_TITLE}', 'chat', '${now}', 2, false, true, 'idle', 'inbox', false, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
      + `VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"compare the fares","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
      + `VALUES ('${randomUUID()}', 'WidgetShown', '{"app_id":"${appId}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    // Showing adds no chip; only a pin does (ADR 0407).
    ...(pinned ? [
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

const chip = (page: Page) => page.locator('.widget-chip:visible', { hasText: WIDGET_NAME }).first();
const card = (page: Page) => page.locator('.chat-exchange .widget-card-inline:visible').first();
const droppedCard = (page: Page) => page.locator('.widget-shelf-drop .widget-card:visible').first();

test.describe('Widgets', () => {
  let threadId: string;
  let appId: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    threadId = randomUUID();
    appId = `e2e-widget-${randomUUID().slice(0, 8)}`;
    await page.goto('/');
  });

  test.afterEach(async ({ page }) => {
    psql(`DELETE FROM events WHERE thread_id = '${threadId}'; DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`);
    for (const file of ['index.html', 'manifest.json']) {
      await apiRequest(page).delete(`/api/v1/data/apps/${appId}/${file}`);
    }
  });

  test('shows the widget inline in its card, drops open from its chip, and survives a reload', async ({ page }) => {
    await writeWidget(page, appId, threadId, SHORT_WIDGET);
    seedThreadShowingWidget(threadId, appId, { pinned: true });
    await openThread(page, threadId);

    await expect(chip(page)).toBeVisible({ timeout: 15_000 });
    await expect(card(page)).toBeVisible();
    await expect(card(page).locator('.widget-bar-name')).toHaveText(WIDGET_NAME);
    await expect(card(page).locator('iframe[data-role="app-ui-frame"]')).toHaveAttribute(
      'src', new RegExp(`/app/${appId}/`),
    );
    // It fits, so the card does not clip it.
    await expect(card(page).locator('.widget-card-fade')).toHaveCount(0);

    // A chip tap drops the widget open under the title, in an app frame.
    expect(await clickVisibleElement(page, '.widget-chip', WIDGET_NAME)).toBe(true);
    await expect(droppedCard(page)).toBeVisible();
    await expect(droppedCard(page).locator('iframe[data-role="app-ui-frame"]')).toHaveAttribute(
      'src', new RegExp(`/app/${appId}/`),
    );

    // The apps list never holds it.
    const apps = await (await page.request.get('/api/v1/apps')).json() as Array<{ id: string }>;
    expect(apps.some((a) => a.id === appId)).toBe(false);

    await page.reload();
    await expect(chip(page)).toBeVisible({ timeout: 15_000 });
    await expect(card(page)).toBeVisible();
  });

  test('an unpinned widget shows inline with no chip', async ({ page }) => {
    await writeWidget(page, appId, threadId, SHORT_WIDGET);
    seedThreadShowingWidget(threadId, appId, { pinned: false });
    await openThread(page, threadId);

    // The name comes from the shelf, so once it shows the shelf has loaded.
    await expect(card(page).locator('.widget-bar-name')).toHaveText(WIDGET_NAME, { timeout: 15_000 });
    await expect(card(page).locator('iframe[data-role="app-ui-frame"]')).toHaveAttribute(
      'src', new RegExp(`/app/${appId}/`),
    );
    await expect(page.locator('.widget-chip:visible')).toHaveCount(0);
  });

  test('a tall widget keeps its full height, clipped, and Expand opens it in Canvas', async ({ page }) => {
    await writeWidget(page, appId, threadId, TALL_WIDGET);
    seedThreadShowingWidget(threadId, appId, { pinned: false });
    await openThread(page, threadId);

    const expand = card(page).locator('.widget-card-fade .action-btn');
    await expect(expand).toBeVisible({ timeout: 15_000 });
    // The frame is as tall as its content, so it has nothing to scroll.
    const frame = card(page).locator('.widget-frame');
    const clip = card(page).locator('.widget-card-clip');
    const [frameBox, clipBox] = await Promise.all([frame.boundingBox(), clip.boundingBox()]);
    expect(frameBox && clipBox).toBeTruthy();
    expect(frameBox!.height).toBeGreaterThan(clipBox!.height);

    await expand.click();
    await expect(page.locator(`.app-ui-inline iframe[data-role="app-ui-frame"][src*="${appId}"]:visible`))
      .toHaveCount(1, { timeout: 15_000 });
  });

  test('the shelf never squeezes the title', async ({ page }) => {
    await writeWidget(page, appId, threadId, SHORT_WIDGET);
    seedThreadShowingWidget(threadId, appId, { pinned: true });
    await openThread(page, threadId);
    await expect(chip(page)).toBeVisible({ timeout: 15_000 });

    const titleRow = isMobileViewport(page) ? '.mobile-thread-title-row' : '.thread-view-header';
    const title = await page.locator(`${titleRow}:visible .thread-title-menu`).first().boundingBox();
    const chipBox = await chip(page).boundingBox();
    expect(title && chipBox).toBeTruthy();
    if (isMobileViewport(page)) {
      // The phone stacks: the shelf has a line of its own under the title.
      expect(chipBox!.y).toBeGreaterThanOrEqual(title!.y + title!.height);
    } else {
      // A desktop row with room keeps the chips on the title's line.
      expect(chipBox!.y).toBeLessThan(title!.y + title!.height);
    }
  });
});
