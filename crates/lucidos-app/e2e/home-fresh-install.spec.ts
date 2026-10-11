import { test, expect, Locator, Page } from './fixtures';
import { assertHealthy, assertUserMessagesVisible, gotoWithRetry, isMobileViewport, waitForWorkspaceReady } from './helpers';
import { clearAllThreads, ensureHomeThread, psql, resetWelcomePreference } from './db-helpers';

/** A fresh install starts in Home (ADR 0411). The first screen is Home in the
 *  compose layout: its title at the top, the welcome above a centred prompt.
 *  The setup interview then runs in Home and creates no thread of its own.
 *
 *  `gotoWithRetry`, never `navigateToApp`, which steps off Home for the specs
 *  that start on the compose view. */
test.describe('A fresh install starts in Home', () => {
  let home: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // A fresh workspace: no thread but Home, which boot made, and a welcome
    // nobody dismissed. Truncating takes Home with it, so it is put back.
    clearAllThreads();
    home = ensureHomeThread();
    resetWelcomePreference();
  });

  /** Press the way the running device would, as welcome.spec.ts explains. */
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
    const welcome = page.locator('.welcome-message:visible');
    await expect(welcome).toHaveCount(1);
    await expect(welcome.getByText('Hi, there!')).toBeVisible();
    await expect(page.locator('.thread-pane.compose-empty:visible')).toHaveCount(1);
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
