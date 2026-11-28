import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, waitForResponse, uniqueMessage,
  assertHealthy, openThreadDrawer, newThread, countVisibleThreadRows,
  clickVisibleElement,
} from './helpers';

async function clickSearchButton(page: import('@playwright/test').Page) {
  await clickVisibleElement(page, 'button[aria-label="Search threads"]');
}

test.describe('Thread search', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('search filters threads by query', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const marker = uniqueMessage('searchable');
    await sendMessage(page, `Say exactly: "The ${marker} thread is active"`);
    await waitForResponse(page);

    await openThreadDrawer(page);

    await clickSearchButton(page);

    const searchInput = page.locator('.thread-search-input:visible').first();
    await expect(searchInput).toBeVisible({ timeout: 5_000 });
    await searchInput.fill(marker);

    await page.waitForFunction(() => {
      const rows = document.querySelectorAll('.thread-row');
      return Array.from(rows).filter(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      }).length >= 1;
    }, undefined, { timeout: 10_000 });

    const threadCount = await countVisibleThreadRows(page);
    expect(threadCount).toBeGreaterThanOrEqual(1);
  });

  test('search with no matches shows empty state', async ({ page }) => {
    await navigateToApp(page);

    await openThreadDrawer(page);

    await clickSearchButton(page);

    const searchInput = page.locator('.thread-search-input:visible').first();
    await expect(searchInput).toBeVisible({ timeout: 5_000 });
    await searchInput.fill('zzzzz-nonexistent-query-99999');

    await page.waitForFunction(() => {
      const rows = document.querySelectorAll('.thread-row');
      return Array.from(rows).filter(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      }).length === 0;
    }, undefined, { timeout: 10_000 });

    const threadCount = await countVisibleThreadRows(page);
    expect(threadCount).toBe(0);
  });

  test('closing search restores full thread list', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    await sendMessage(page, `Say exactly: "search-close-test"`);
    await waitForResponse(page);

    await openThreadDrawer(page);

    await page.waitForFunction(() => {
      const rows = document.querySelectorAll('.thread-row');
      return Array.from(rows).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 10_000 });

    const beforeCount = await countVisibleThreadRows(page);

    await clickSearchButton(page);

    const searchInput = page.locator('.thread-search-input:visible').first();
    await searchInput.fill('zzzzz-nonexistent');

    await page.waitForFunction(() => {
      const rows = document.querySelectorAll('.thread-row');
      return Array.from(rows).filter(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      }).length === 0;
    }, undefined, { timeout: 10_000 });

    const closed = await clickVisibleElement(page, 'button[aria-label="Close search"]');
    expect(closed).toBe(true);

    await expect(page.locator('.threads-header-title:visible, .mobile-header-title:visible').first()).toBeVisible({ timeout: 5_000 });

    await page.waitForFunction(() => {
      const rows = document.querySelectorAll('.thread-row');
      return Array.from(rows).filter(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      }).length >= 1;
    }, undefined, { timeout: 10_000 });

    const afterCount = await countVisibleThreadRows(page);
    expect(afterCount).toBeGreaterThanOrEqual(1);
  });

  test('escape key closes search', async ({ page }) => {
    await navigateToApp(page);

    await openThreadDrawer(page);

    await clickSearchButton(page);

    const searchInput = page.locator('.thread-search-input:visible').first();
    await expect(searchInput).toBeVisible({ timeout: 5_000 });
    await searchInput.fill('test');

    await searchInput.press('Escape');

    await expect(page.locator('.threads-header-title:visible, .mobile-header-title:visible').first()).toBeVisible({ timeout: 5_000 });
  });
});
