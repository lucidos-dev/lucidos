import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  waitForResponse,
  uniqueMessage,
  waitForThreadTitle,
} from './helpers';

async function readBand(page: import('@playwright/test').Page) {
  return page.evaluate(() => {
    const probe = (token: string) => {
      const el = document.createElement('div');
      el.style.background = `var(${token})`;
      document.body.appendChild(el);
      const color = getComputedStyle(el).backgroundColor;
      el.remove();
      return color;
    };
    const row = Array.from(document.querySelectorAll<HTMLElement>('.mobile-thread-title-row'))
      .find((r) => r.getBoundingClientRect().width > 0);
    if (!row) return null;
    return {
      background: getComputedStyle(row).backgroundColor,
      hairline: getComputedStyle(row, '::before').content,
      page: probe('--bg-primary'),
      band: probe('--bg-secondary'),
    };
  });
}

test.describe('Mobile thread title band', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('centers the title with the Users badge and opens the panel below it', async ({ page }) => {
    const driver = { id: 'sam', name: 'Sam' };
    const users = { users: [{ ...driver, has_device: true }], you: driver.id, team: true };
    await page.route('**/api/v1/users', (route) => route.fulfill({ json: users }));
    await page.route('**/api/v1/bootstrap', async (route) => {
      const response = await route.fetch();
      const snapshot = await response.json();
      snapshot.users = { data: users };
      const headers = { ...response.headers() };
      delete headers.etag;
      await route.fulfill({ response, headers, json: snapshot });
    });
    await page.route('**/api/v1/threads/*/users', (route) => route.fulfill({
      json: { root_thread_id: route.request().url().split('/').at(-2), driver, watchers: [] },
    }));
    await navigateToApp(page);
    await sendMessage(page, `Say exactly: "${uniqueMessage('users')}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    const row = page.locator('.mobile-swipe-pane .mobile-thread-title-row');
    const title = row.locator('.thread-title-menu');
    const badge = row.getByRole('button', { name: 'Users in this thread' });
    await expect(badge).toBeVisible();
    const titleBox = (await title.boundingBox())!;
    const badgeBox = (await badge.boundingBox())!;
    expect(Math.abs(titleBox.y + titleBox.height / 2 - badgeBox.y - badgeBox.height / 2)).toBeLessThan(1);

    await badge.click();
    const panel = row.locator('[data-role="thread-users-panel"]');
    await expect(panel.getByRole('button', { name: 'Edit users' })).toBeInViewport();
    await expect.poll(async () => (await panel.boundingBox())?.y).toBeGreaterThanOrEqual(badgeBox.y + badgeBox.height - 1);
    expect(await title.boundingBox()).toEqual(titleBox);
    expect(await badge.boundingBox()).toEqual(badgeBox);

    await badge.click();
    await expect(panel).toHaveCount(0);
    expect(await title.boundingBox()).toEqual(titleBox);
  });

  test('rests on the page with no hairline, and bands once scrolled', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('band');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    const rest = await readBand(page);
    expect(rest, 'visible .mobile-thread-title-row not found').not.toBeNull();
    expect(rest!.background).toBe(rest!.page);
    expect(rest!.hairline).toBe('none');

    await page.evaluate(() => {
      document.querySelectorAll('.mobile-thread-title-row').forEach((r) => r.classList.add('scrolled'));
    });
    await expect.poll(async () => {
      const b = await readBand(page);
      return b && b.background === b.band;
    }).toBe(true);
    expect((await readBand(page))!.hairline).toBe('none');
  });
});
