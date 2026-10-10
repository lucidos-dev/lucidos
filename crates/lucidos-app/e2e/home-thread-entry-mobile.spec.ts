import { randomUUID } from 'crypto';
import { test, expect, type Page } from './fixtures';
import { navigateToApp, assertHealthy, ensureMobileView, apiRequest } from './helpers';
import { ensureHomeThread, psql } from './db-helpers';

/** A phone reaches the home thread from a Home icon paired with the mark in the
 *  thread pane's header. The thread drawer's header has no Home icon, so the
 *  menu its mark opens keeps a Home row; the thread pane's menu drops it. */
test.describe('The home thread from the phone header', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // A spec before this one may have truncated the threads, home included.
    ensureHomeThread();
    await navigateToApp(page);
    await ensureMobileView(page, 'thread');
  });

  test('Home and the mark centre together, and Home opens it', async ({ page }) => {
    const home = page.locator('.mobile-thread-header .header-mark-pair .home-thread-btn');
    await expect(home).toBeVisible();

    const offset = await page.evaluate(() => {
      const row = document.querySelector('.mobile-thread-header .mobile-header-row')!.getBoundingClientRect();
      const pair = document.querySelector('.mobile-thread-header .header-mark-pair')!.getBoundingClientRect();
      return (pair.left + pair.right) / 2 - (row.left + row.right) / 2;
    });
    expect(Math.abs(offset), `the pair sits ${offset}px off the row middle`).toBeLessThan(2.5);

    await home.click();
    await expect(page.locator('.thread-title-menu:visible')).toHaveText('Home');
  });

  test('Home and the mark spread apart when the row has room, and touch when it has none', async ({ page }) => {
    // The app writes the scale itself once preferences load, so wait for that
    // write before ours, or ours is reverted under the measurement.
    await page.waitForFunction(() => localStorage.getItem('lucidos-ui-scale') !== null, undefined, { timeout: 10_000 });
    await expect(page.locator('.mobile-thread-header .header-mark-pair .home-thread-btn')).toBeVisible();

    const measure = () => page.evaluate(() => {
      const header = document.querySelector('.mobile-thread-header')!;
      const box = (sel: string) => header.querySelector(sel)!.getBoundingClientRect();
      const probe = document.createElement('div');
      probe.style.width = 'var(--header-mark-pair-gap-max)';
      document.body.appendChild(probe);
      const max = probe.getBoundingClientRect().width;
      probe.remove();
      const row = box('.mobile-header-row');
      const pair = box('.header-mark-pair');
      const home = box('.home-thread-btn');
      const mark = box('.brand-mark-slot');
      return {
        max,
        gap: mark.left - home.right,
        offset: (pair.left + pair.right) / 2 - (row.left + row.right) / 2,
        backClearance: pair.left - box('button[aria-label="Previous thread"]').right,
        forwardClearance: box('button[aria-label="Next thread"]').left - pair.right,
      };
    });

    for (const [scale, rootPx, roomy] of [[100, 16, true], [200, 32, false]] as const) {
      await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`), scale);
      await expect
        .poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize))
        .toBe(`${rootPx}px`);
      const m = await measure();
      if (roomy) {
        expect(m.gap, `ui-scale ${scale}: gap ${m.gap.toFixed(1)} is not the max ${m.max.toFixed(1)}`).toBeCloseTo(m.max, 0);
      } else {
        expect(m.gap, `ui-scale ${scale}: gap ${m.gap.toFixed(1)} did not close up`).toBeCloseTo(0, 0);
      }
      expect(Math.abs(m.offset), `ui-scale ${scale}: the pair sits ${m.offset}px off the row middle`).toBeLessThan(2.5);
      expect(m.backClearance, `ui-scale ${scale}: the back chevron overlaps the pair`).toBeGreaterThan(0);
      expect(m.forwardClearance, `ui-scale ${scale}: the forward chevron overlaps the pair`).toBeGreaterThan(0);
    }
  });

  test('only the thread drawer\'s menu carries a Home row', async ({ page }) => {
    const homeRow = page.locator('.brand-menu .home-thread-btn');

    await page.locator('.mobile-thread-header [data-role="brand-menu-toggle"]').click();
    await expect(page.locator('.brand-menu')).toBeVisible();
    await expect(homeRow).toHaveCount(0);
    await page.keyboard.press('Escape');
    await expect(page.locator('.brand-menu')).toBeHidden();

    await ensureMobileView(page, 'threads');
    await page.locator('.mobile-threads-header [data-role="brand-menu-toggle"]').click();
    const first = page.locator('.brand-menu [role="menuitem"]').first();
    await expect(first).toHaveText('Home');
    await first.click();
    await expect(page.locator('.brand-menu')).toBeHidden();
    await expect(page.locator('.thread-title-menu:visible')).toHaveText('Home');
  });

  test('holding Home floats a pinned widget over the open thread, where it stays', async ({ page }) => {
    const home = ensureHomeThread();
    const other = randomUUID();
    const appId = `e2e-home-widget-${randomUUID().slice(0, 8)}`;
    const written = [`apps/${appId}/index.html`, `apps/${appId}/manifest.json`];
    const writeFile = async (path: string, body: string) => {
      const resp = await apiRequest(page).put(`/api/v1/data/${path}`, { headers: { 'Content-Type': 'text/plain' }, data: body });
      expect(resp.ok(), `PUT ${path}`).toBeTruthy();
    };
    const now = new Date().toISOString();
    const event = (type: string, threadId: string, payload: string) =>
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
      + `VALUES ('${randomUUID()}', '${type}', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;
    try {
      await writeFile(written[0], '<!doctype html><html><head><script src="/api/v1/sdk.js"></script></head><body><h1>Fares</h1></body></html>');
      await writeFile(written[1], JSON.stringify({ name: 'E2E home fares', kind: 'widget', origin_thread_id: home }));
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) `
          + `VALUES ('${other}', 'E2E elsewhere', 'chat', '${now}', 1, false, true, 'idle', 'inbox', false, 0, 'none', false)`,
        event('MessageReceived', other, '{"text":"somewhere else","mode":"human","channel":"chat"}'),
        event('WidgetShown', home, `{"app_id":"${appId}"}`),
        event('WidgetPinned', home, `{"app_id":"${appId}"}`),
      ].join(';\n'));
      await page.evaluate((id) => localStorage.setItem('lucidos-focused-thread', id), other);
      await page.reload();
      await ensureMobileView(page, 'thread');
      const title = page.locator('.thread-title-menu:visible');
      await expect(title).toHaveText('E2E elsewhere', { timeout: 15_000 });

      await holdCentre(page, page.locator('.mobile-thread-header .home-thread-btn'));
      // An instance key is the app id plus its params (ADR 0415).
      const row = page.locator(`.thread-overflow-menu [data-home-widget^="${appId}?"]`);
      // The row's text also holds the icon's monogram, which is aria-hidden.
      await expect(row.locator('.thread-overflow-label')).toHaveText('E2E home fares');
      await expect(title).toHaveText('E2E elsewhere');
      await row.click();

      const win = page.locator(`[data-widget-window^="${appId}?"]`);
      await expect(win).toBeVisible();
      await expect(win.locator('iframe[data-role="app-ui-frame"]')).toHaveAttribute('src', new RegExp(`/app/${appId}/`));
      await expect(title).toHaveText('E2E elsewhere');

      // A drag on the bar moves it.
      const before = (await win.boundingBox())!;
      const bar = (await win.locator('.widget-bar-name').boundingBox())!;
      await page.mouse.move(bar.x + bar.width / 2, bar.y + bar.height / 2);
      await page.mouse.down();
      await page.mouse.move(bar.x + bar.width / 2, bar.y + bar.height / 2 + 120, { steps: 8 });
      await page.mouse.up();
      await expect.poll(async () => (await win.boundingBox())!.y - before.y).toBeGreaterThan(100);

      // A tap elsewhere leaves it open; its Close shuts it.
      await page.locator('.thread-content:visible').click({ position: { x: 20, y: 20 }, force: true });
      await expect(win).toBeVisible();
      await win.locator('button[aria-label="Close E2E home fares"]').click();
      await expect(win).toHaveCount(0);
    } finally {
      psql(`DELETE FROM events WHERE thread_id = '${other}' OR (thread_id = '${home}' AND payload->>'app_id' = '${appId}'); `
        + `DELETE FROM thread_summaries WHERE thread_id = '${other}'`);
      for (const path of written) await apiRequest(page).delete(`/api/v1/data/${path}`);
    }
  });
});

/** A still press on the element's centre, long enough to count as a hold.
 *  Playwright cannot hold a touchscreen tap, and `useLongPress` reads no
 *  `pointerType`, so a held mouse button is the same gesture. */
async function holdCentre(page: Page, target: ReturnType<Page['locator']>): Promise<void> {
  const box = (await target.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(700);
  await page.mouse.up();
}

