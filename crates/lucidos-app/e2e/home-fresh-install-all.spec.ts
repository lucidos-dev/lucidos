import { test, expect, Locator, Page } from './fixtures';
import { assertHealthy, assertUserMessagesVisible, gotoWithRetry, isMobileViewport, waitForWorkspaceReady } from './helpers';
import { clearAllThreads, ensureHomeThread, psql, resetWelcomePreference } from './db-helpers';

test.describe('A fresh install starts in Home', () => {
  let home: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearAllThreads();
    home = ensureHomeThread();
    resetWelcomePreference();
  });

  async function press(page: Page, button: Locator): Promise<void> {
    if (isMobileViewport(page)) await button.tap();
    else await button.click();
  }

  test('opens Home, holding the welcome', async ({ page }) => {
    await gotoWithRetry(page, '/');
    await waitForWorkspaceReady(page);

    await expect
      .poll(() => page.evaluate(() => localStorage.getItem('lucidos-focused-thread')), { timeout: 10_000 })
      .toBe(home);
    await expect(page.locator('.home-welcome-title .thread-title-menu:visible')).toHaveText('Home');
    await expect(page.locator('.thread-pane.compose-empty:visible')).toHaveCount(1);
    await expect(page.locator('.home-pane')).toHaveCount(0);
    const welcome = page.locator('.welcome-message:visible');
    await expect(welcome).toHaveCount(1);
    await expect(welcome.getByText('Hi, there!')).toBeVisible();
  });

  test('runs the setup interview in Home', async ({ page }) => {
    await gotoWithRetry(page, '/');
    await waitForWorkspaceReady(page);
    const start = page.locator('.welcome-setup-interview-btn:visible').first();
    await expect(start).toBeVisible({ timeout: 10_000 });

    await press(page, start);

    await assertUserMessagesVisible(page, ['Help me get the most out of Lucidos']);
    await expect(page.locator('.welcome-message')).toHaveCount(0);
    expect(await page.evaluate(() => localStorage.getItem('lucidos-focused-thread'))).toBe(home);
    await expect
      .poll(() => psql(
        `SELECT count(*) FROM events WHERE thread_id = '${home}' AND event_type = 'MessageReceived'`,
      ), { timeout: 10_000 })
      .toBe('1');
    expect(psql('SELECT count(*) FROM thread_summaries WHERE NOT is_home')).toBe('0');
  });
});
