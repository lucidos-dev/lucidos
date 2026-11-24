import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';

const SHOOTING = process.env.HEADER_SHOTS === '1';
const DIR = 'test-results/header-shots';

test.use({ viewport: { width: 1280, height: 800 } });

function markConn(page: Page): Promise<string | null> {
  return page.evaluate(() =>
    document.querySelector('.desktop-header [data-role="brand-menu-toggle"]')?.getAttribute('data-conn') ?? null);
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

test.describe('Desktop header shots', () => {
  test.skip(!SHOOTING, 'set HEADER_SHOTS=1 to take screenshots');

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('the bar, the menu, and every connection state', async ({ page, context }) => {
    await navigateToApp(page);
    await waitForSplashGone(page);
    await expect.poll(() => markConn(page)).toBe('connected');

    await shootHeader(page, 'desktop-1-bar');

    await page.locator('.desktop-header [data-role="brand-menu-toggle"]').click();
    await expect(page.locator('.brand-menu')).toBeVisible();
    await page.screenshot({ path: `${DIR}/desktop-2-menu.png` });
    await page.keyboard.press('Escape');
    await expect(page.locator('.brand-menu')).toHaveCount(0);

    await page.evaluate(() => {
      const label = document.querySelector('.desktop-header .pane-header-brand-label') as HTMLElement;
      const mark = label.querySelector('.brand-mark') as HTMLElement;
      const strip = document.createElement('div');
      strip.className = 'conn-strip';
      strip.style.cssText = 'display:flex;align-items:center;gap:2rem;margin-left:2rem';
      for (const state of ['connected', 'connecting', 'disconnected']) {
        const cell = document.createElement('div');
        cell.style.cssText = 'display:flex;align-items:center;gap:0.4rem;color:#fff;font:600 12px system-ui';
        const clone = mark.cloneNode(true) as HTMLElement;
        clone.setAttribute('data-conn', state);
        const caption = document.createElement('span');
        caption.textContent = state;
        cell.append(clone, caption);
        strip.append(cell);
      }
      label.append(strip);
      label.style.width = 'auto';
      label.style.justifyContent = 'flex-start';
    });
    await page.waitForTimeout(300);
    await shootHeader(page, 'desktop-4-conn-strip');
    await page.reload();
    await waitForSplashGone(page);

    await context.setOffline(true);
    await expect.poll(() => markConn(page), { timeout: 30_000 }).toBe('disconnected');
    await page.waitForTimeout(300);
    await shootHeader(page, 'desktop-5-conn-disconnected');

    await context.setOffline(false);
    await expect.poll(() => markConn(page), { timeout: 30_000 }).toBe('connected');
    await shootHeader(page, 'desktop-6-conn-connected');
  });
});
