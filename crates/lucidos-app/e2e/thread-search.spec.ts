import { test, expect } from '@playwright/test';
import {
  navigateToApp, sendMessage, waitForResponse, uniqueMessage,
  assertHealthy, openThreadDrawer, newThread, countVisibleThreadRows,
  clickVisibleElement,
} from './helpers';

/** Click the visible "Search threads" button (handles dual desktop/mobile layout) */
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

    // Wait for at least one result to appear
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

    // Wait for results to settle (debounce + render)
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

    // Wait for threads to load
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

    // Close search (dual-layout: click the visible close button).
    // Verify the button was actually clicked — if not, the search wasn't open.
    const closed = await clickVisibleElement(page, 'button[aria-label="Close search"]');
    expect(closed).toBe(true);

    // Wait for header title to reappear (confirms search actually closed).
    // Can't check search input dimensions — clip-path:inset(50%) hides visually
    // but getBoundingClientRect() still returns non-zero dimensions.
    await expect(page.locator('.threads-header-title:visible, .mobile-header-title:visible').first()).toBeVisible({ timeout: 5_000 });

    // Wait for thread list to be restored — at least 1 thread visible
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
