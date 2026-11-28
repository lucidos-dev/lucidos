import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, clickVisibleElement, navigateToApp, waitForEventStream } from './helpers';

const CONTENT_TITLE = '.pane-header-content-title';
const HISTORY_OPTION = '.nav-history-menu .dropdown-option';

async function navigateMenu(page: import('@playwright/test').Page, target: string): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate {${target}} -> ${res.status()}`).toBeTruthy();
}

async function waitForContentTitle(page: import('@playwright/test').Page, text: string): Promise<void> {
  await expect(page.locator(`${CONTENT_TITLE}:visible`).first()).toContainText(text, { timeout: 10_000 });
}

test.describe('back chevron history menu', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('right-click on Back lists the back-history and jumps multiple steps at once', async ({ page }) => {
    await navigateToApp(page);
    await waitForEventStream(page);

    await navigateMenu(page, 'apps');
    await waitForContentTitle(page, 'Apps');
    await navigateMenu(page, 'triggers');
    await waitForContentTitle(page, 'Triggers');
    await navigateMenu(page, 'notifications');
    await waitForContentTitle(page, 'Notifications');

    const backBtn = page.locator('.content-back-btn:visible').first();
    await expect(backBtn).toBeEnabled();
    await backBtn.click({ button: 'right' });

    await page.waitForFunction((sel) => {
      const opts = Array.from(document.querySelectorAll(sel));
      return opts.some(el => (el.textContent ?? '').includes('Triggers'))
        && opts.some(el => (el.textContent ?? '').includes('Apps'));
    }, HISTORY_OPTION, { timeout: 10_000 });

    await page.waitForFunction((sel) => {
      const btn = Array.from(document.querySelectorAll('.content-back-btn'))
        .find(el => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; }) as HTMLElement | undefined;
      const menu = document.querySelector(sel) as HTMLElement | null;
      if (!btn || !menu) return false;
      const b = btn.getBoundingClientRect();
      const m = menu.getBoundingClientRect();
      const inViewport = m.left >= -1 && m.top >= -1
        && m.right <= window.innerWidth + 1 && m.bottom <= window.innerHeight + 1;
      return inViewport
        && Math.abs(m.left - b.left) <= 8
        && Math.abs(m.top - b.bottom) <= 12;
    }, '.nav-history-menu', { timeout: 10_000 });

    const geom = await page.evaluate((sel) => {
      const btn = Array.from(document.querySelectorAll('.content-back-btn'))
        .find(el => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; }) as HTMLElement | undefined;
      const menu = document.querySelector(sel) as HTMLElement | null;
      if (!btn || !menu) return null;
      const b = btn.getBoundingClientRect();
      const m = menu.getBoundingClientRect();
      return {
        b, m,
        inViewport: m.left >= -1 && m.top >= -1
          && m.right <= window.innerWidth + 1 && m.bottom <= window.innerHeight + 1,
      };
    }, '.nav-history-menu');
    expect(geom, 'found Back button + menu rects').not.toBeNull();
    expect(geom!.inViewport, `menu rect ${JSON.stringify(geom!.m)} within viewport`).toBeTruthy();
    expect(Math.abs(geom!.m.left - geom!.b.left)).toBeLessThanOrEqual(8);
    expect(Math.abs(geom!.m.top - geom!.b.bottom)).toBeLessThanOrEqual(12);

    const jumped = await clickVisibleElement(page, HISTORY_OPTION, 'Apps');
    expect(jumped, 'history option "Apps" was clickable').toBeTruthy();

    await waitForContentTitle(page, 'Apps');

    await page.waitForFunction((sel) => {
      return !Array.from(document.querySelectorAll(sel)).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, HISTORY_OPTION, { timeout: 5_000 });

    const fwdBtn = page.locator('.content-forward-btn:visible').first();
    await expect(fwdBtn).toBeEnabled();
    await fwdBtn.click({ button: 'right' });
    await page.waitForFunction((sel) => {
      const opts = Array.from(document.querySelectorAll(sel));
      return opts.some(el => (el.textContent ?? '').includes('Triggers'))
        && opts.some(el => (el.textContent ?? '').includes('Notifications'));
    }, HISTORY_OPTION, { timeout: 10_000 });

    const jumpedFwd = await clickVisibleElement(page, HISTORY_OPTION, 'Notifications');
    expect(jumpedFwd, 'forward history option "Notifications" was clickable').toBeTruthy();
    await waitForContentTitle(page, 'Notifications');
  });
});
