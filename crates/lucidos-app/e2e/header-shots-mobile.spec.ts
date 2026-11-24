import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp, ensureMobileView, disableMobileDynamicBars } from './helpers';

const SHOOTING = process.env.HEADER_SHOTS === '1';
const DIR = 'test-results/header-shots';

function markConn(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const el = document.querySelector('.mobile-thread-header [data-role="brand-menu-toggle"]');
    return el?.getAttribute('data-conn') ?? null;
  });
}

async function waitForSplashGone(page: Page): Promise<void> {
  await expect.poll(
    () => page.evaluate(() => document.querySelectorAll('.boot-splash').length),
    { timeout: 30_000 },
  ).toBe(0);
}

async function shootHeader(page: Page, name: string): Promise<void> {
  await page.locator('.app-header').screenshot({ path: `${DIR}/${name}.png` });
}

test.describe('Header shots', () => {
  test.skip(!SHOOTING, 'set HEADER_SHOTS=1 to take screenshots');

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await disableMobileDynamicBars(page);
  });

  test('every pane and both connection states', async ({ page, context }) => {
    await navigateToApp(page);

    await waitForSplashGone(page);
    await ensureMobileView(page, 'thread');
    await expect.poll(() => markConn(page)).toBe('connected');
    await shootHeader(page, '1-thread-connected');

    await page.locator('[data-role="brand-menu-toggle"]:visible').first().tap();
    await expect(page.locator('.brand-menu')).toBeVisible();
    await page.screenshot({ path: `${DIR}/2-menu-open.png` });
    await page.keyboard.press('Escape');
    await expect(page.locator('.brand-menu')).toHaveCount(0);

    await ensureMobileView(page, 'threads');
    await shootHeader(page, '3-threads-drawer-mark');

    await ensureMobileView(page, 'content');
    await shootHeader(page, '4-content-chevrons');

    await ensureMobileView(page, 'thread');
    await context.setOffline(true);
    await expect.poll(() => markConn(page), { timeout: 30_000 }).toBe('disconnected');
    await page.waitForTimeout(300);
    await shootHeader(page, '5-disconnected');
    await page.screenshot({ path: `${DIR}/6-disconnected-full.png` });

    await context.setOffline(false);
    await expect.poll(() => markConn(page), { timeout: 30_000 }).toBe('connected');
  });
});
