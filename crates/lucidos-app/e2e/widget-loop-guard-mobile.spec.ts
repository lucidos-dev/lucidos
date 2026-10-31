import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, gotoWithRetry, hideDashboardRail, navigateToApp } from './helpers';
import { ensureHomeThread, psql } from './db-helpers';

/**
 * A widget cannot lock a phone out of the app
 * (`docs/plans/2026-10-18-widget-loop-and-restore-guard.md`). WebKit runs the
 * app frame in the page's process, so a widget that loops hangs the whole
 * page there. The `mobile-webkit` project is the one that proves these.
 *
 * - A widget whose `<html>` observer writes back to `<html>` still sizes its
 *   frame, because the SDK writes only when the host's value changes.
 * - A widget whose content grows with its frame stops growing, because the
 *   widget height guard locks the frame.
 * - A launch that finds the restore guard's marker starts on Home and offers
 *   the thread back.
 */

const DEFAULT_WIDGET_HEIGHT_PX = 240;
const MESSAGE_TEXT = 'compare the fares';

let written: string[] = [];
let seededThreads: string[] = [];

async function writeFile(page: Page, path: string, body: string): Promise<void> {
  written.push(path);
  const resp = await apiRequest(page).put(`/api/v1/data/${path}`, {
    headers: { 'Content-Type': 'text/plain' },
    data: body,
  });
  expect(resp.ok(), `PUT ${path}`).toBeTruthy();
}

async function writeWidget(page: Page, appId: string, threadId: string, html: string): Promise<void> {
  await writeFile(page, `apps/${appId}/index.html`, html);
  await writeFile(page, `apps/${appId}/manifest.json`, JSON.stringify({
    name: 'E2E loop widget',
    description: 'A widget that writes to its own root',
    kind: 'widget',
    origin_thread_id: threadId,
  }));
}

/** A thread with one human message, showing `appId` inline when given. */
function seedThread(threadId: string, appId?: string): void {
  seededThreads.push(threadId);
  const now = new Date().toISOString();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) `
      + `VALUES ('${threadId}', 'E2E widget loop guard', 'chat', '${now}', 2, false, true, 'idle', 'inbox', false, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
      + `VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"${MESSAGE_TEXT}","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ...(appId ? [
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
        + `VALUES ('${randomUUID()}', 'WidgetShown', '{"app_id":"${appId}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ] : []),
  ].join(';\n'));
}

async function openThread(page: Page, threadId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  await navigateToApp(page);
}

function widgetHtml(head: string, body: string): string {
  return `<!doctype html><html><head><script src="/api/v1/sdk.js"></script>${head}</head><body>${body}</body></html>`;
}

/** The thread-cost widget's preview pin: it re-adds a present class, which
 *  still queues a mutation record, on every write to `<html>`. */
const SELF_PINNING_WIDGET = widgetHtml(
  `<script>
    function pin() {
      const el = document.documentElement;
      el.classList.add('preview');
      if (el.dataset.widgetSize !== 'small') el.dataset.widgetSize = 'small';
    }
    pin();
    new MutationObserver(pin).observe(document.documentElement, { attributes: true, attributeFilter: ['data-widget-size', 'class'] });
  </script>`,
  '<div style="height: 400px">Cost</div>',
);

/** Content as tall as the frame plus the body margin, so each report grows
 *  the frame by 16 px. */
const VIEWPORT_TALL_WIDGET = widgetHtml('<style>body { height: 100vh; }</style>', '<p>Cost</p>');

const frameHeight = (page: Page) => page.locator('.chat-exchange .widget-frame:visible').first().evaluate(
  (el) => parseFloat((el as HTMLElement).style.height),
);

test.describe('A widget cannot lock a phone out of the app', () => {
  let appId: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    written = [];
    seededThreads = [];
    appId = `e2e-loop-widget-${randomUUID().slice(0, 8)}`;
    await hideDashboardRail(page);
    await page.goto('/');
  });

  test.afterEach(async ({ page }) => {
    for (const id of seededThreads) {
      psql(`DELETE FROM events WHERE thread_id = '${id}'; DELETE FROM thread_summaries WHERE thread_id = '${id}'`);
    }
    for (const path of written) {
      await apiRequest(page).delete(`/api/v1/data/${path}`);
    }
  });

  test('a widget that writes back to its root still sizes its frame', async ({ page }) => {
    const threadId = randomUUID();
    await writeWidget(page, appId, threadId, SELF_PINNING_WIDGET);
    seedThread(threadId, appId);
    await openThread(page, threadId);

    await expect(page.locator('.chat-exchange .widget-frame:visible').first()).toBeVisible({ timeout: 15_000 });
    // A loop starves the frame's animation frames, so it never reports and
    // keeps the default height.
    await expect.poll(() => frameHeight(page), { timeout: 15_000 }).not.toBe(DEFAULT_WIDGET_HEIGHT_PX);
    const height = await frameHeight(page);
    expect(height).toBeGreaterThan(400);
    expect(height).toBeLessThan(500);
  });

  test('a widget whose content grows with its frame stops growing', async ({ page }) => {
    const threadId = randomUUID();
    await writeWidget(page, appId, threadId, VIEWPORT_TALL_WIDGET);
    seedThread(threadId, appId);
    // A locked runaway returns to its start height. Only the log line tells
    // that lock from a frame that never reported.
    const locked = page.waitForRequest((req) => req.url().includes('/internal/client-log')
      && req.postDataJSON()?.message === 'height guard locked a frame'
      && req.postDataJSON()?.data?.reason === 'runaway-growth', { timeout: 15_000 });
    await openThread(page, threadId);
    await locked;

    // A locked frame stops moving: two samples apart must agree.
    await expect.poll(async () => {
      const before = await frameHeight(page);
      await page.waitForTimeout(1_500);
      return (await frameHeight(page)) - before;
    }, { timeout: 20_000 }).toBe(0);
    expect(await frameHeight(page)).toBeLessThanOrEqual(DEFAULT_WIDGET_HEIGHT_PX + 16 * 12);
  });

  test('a launch after a crash on a thread starts on Home and offers it back', async ({ page }) => {
    const home = ensureHomeThread();
    const threadId = randomUUID();
    seedThread(threadId);
    // The marker naming the stored focus is what a session that died there
    // leaves behind. The init script runs on every load, so never reload.
    await page.addInitScript((tid: string) => {
      localStorage.setItem('lucidos-focused-thread', tid);
      localStorage.setItem('lucidos-restore-guard', tid);
    }, threadId);
    await gotoWithRetry(page, '/');

    await expect.poll(() => page.evaluate(() => localStorage.getItem('lucidos-focused-thread')), { timeout: 15_000 })
      .toBe(home);
    const toast = page.locator('.toast', { hasText: 'Started on Home' });
    await expect(toast).toBeVisible();

    // A toast with one action takes the tap on the whole card.
    await expect(toast).toHaveAttribute('data-toast-tap', 'action');
    await toast.locator('.toast-title').click();
    await expect(page.locator('.chat-exchange:visible', { hasText: MESSAGE_TEXT }).first()).toBeVisible();
    // The marker names the thread, then the window that armed it (ADR 0585).
    await expect.poll(() => page.evaluate(() => localStorage.getItem('lucidos-restore-guard')?.split(' ')[0]))
      .toBe(threadId);
  });
});
