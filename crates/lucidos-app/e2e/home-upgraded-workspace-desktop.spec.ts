import { randomUUID } from 'crypto';
import { test, expect } from './fixtures';
import { assertHealthy, gotoWithRetry, waitForWorkspaceReady } from './helpers';
import { clearAllThreads, ensureHomeThread, psql, resetWelcomePreference, seedThreadRow } from './db-helpers';

test.describe('An upgraded workspace that had Home off', () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  let home: string;
  const lastThread = randomUUID();

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearAllThreads();
    home = ensureHomeThread();
    resetWelcomePreference();
    const now = new Date().toISOString();
    psql(seedThreadRow({ id: lastThread, title: 'Last week', now, archiveState: 'inbox' }));
    const cost = JSON.stringify({
      producer: 'auxiliary',
      model: 'test-model',
      context_window: 1000,
      sections: [],
      estimated_total_tokens: 120,
      purpose: 'memory',
    });
    psql(
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
      `VALUES ('${randomUUID()}', 'ContextCaptured', '${cost}'::jsonb, '${now}', 'thread', '${home}', '${home}')`,
    );
    await page.addInitScript((id) => {
      localStorage.setItem('lucidos-focused-thread', id);
    }, lastThread);
  });

  test('keeps the stored thread open, and Home shows with the welcome', async ({ page }) => {
    await gotoWithRetry(page, '/');
    await waitForWorkspaceReady(page);

    await expect(page.locator('.thread-title-menu:visible')).toHaveText('Last week');
    expect(await page.evaluate(() => localStorage.getItem('lucidos-focused-thread'))).toBe(lastThread);

    const more = page.locator('.desktop-header .thread-header-more');
    if (await page.locator('.desktop-header .home-thread-btn').count() === 0) await more.click();
    await page.locator('.desktop-header .home-thread-btn, .thread-overflow-item.home-thread-btn').click();

    await expect(page.locator('.home-welcome-title .thread-title-menu:visible')).toHaveText('Home');
    await expect(page.locator('.welcome-message:visible').getByText('Hi, there!')).toBeVisible();
  });
});
