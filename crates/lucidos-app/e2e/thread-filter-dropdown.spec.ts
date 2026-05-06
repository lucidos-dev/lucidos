import { test, expect } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer, clickVisibleElement, waitForVisibleElement } from './helpers';

test.describe('Thread filter dropdown dismiss', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('clicking a thread row while filter is open closes the panel and does NOT focus the row', async ({ page }) => {
    await navigateToApp(page);

    // Need at least two threads so we can click one that isn't currently focused.
    const msg1 = uniqueMessage('filter-dismiss-1');
    await sendMessage(page, `say "${msg1}"`);
    await waitForResponse(page);

    await newThread(page);
    const msg2 = uniqueMessage('filter-dismiss-2');
    await sendMessage(page, `say "${msg2}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);

    // After sending the second message we expect the second thread to be focused.
    const initiallyFocused = await page.evaluate(() => {
      const focused = document.querySelector('.thread-row-focused');
      return focused?.getAttribute('data-thread-nav') ?? null;
    });
    expect(initiallyFocused).not.toBeNull();

    // Open the filter panel (dual-layout safe — click whichever copy is visible).
    await waitForVisibleElement(page, 'button[aria-label="Filter threads"]');
    await clickVisibleElement(page, 'button[aria-label="Filter threads"]');
    await expect(page.locator('.thread-filter-dropdown:visible').first()).toBeVisible();

    // Click on a thread row that is NOT the currently focused one.
    const clickedId = await page.evaluate((focusedId) => {
      const rows = document.querySelectorAll('.thread-row');
      for (const row of rows) {
        const id = row.getAttribute('data-thread-nav');
        const rect = row.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0 && id && id !== focusedId) {
          (row as HTMLElement).click();
          return id;
        }
      }
      return null;
    }, initiallyFocused);
    expect(clickedId).not.toBeNull();
    expect(clickedId).not.toBe(initiallyFocused);

    // The filter panel must close.
    await expect(page.locator('.thread-filter-dropdown')).toHaveCount(0);

    // The focused thread must NOT have changed — the click was consumed by the dismiss.
    const stillFocused = await page.evaluate(() => {
      const focused = document.querySelector('.thread-row-focused');
      return focused?.getAttribute('data-thread-nav') ?? null;
    });
    expect(stillFocused).toBe(initiallyFocused);
  });

  test('clicking the copy button while filter is open closes the panel and does NOT copy', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('filter-dismiss-copy');
    await sendMessage(page, `say "${msg}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);

    // Open the filter panel (dual-layout safe — click whichever copy is visible).
    await waitForVisibleElement(page, 'button[aria-label="Filter threads"]');
    await clickVisibleElement(page, 'button[aria-label="Filter threads"]');
    await expect(page.locator('.thread-filter-dropdown:visible').first()).toBeVisible();

    // Click on a copy button — this normally copies a thread reference and shows a toast.
    const clicked = await page.evaluate(() => {
      const buttons = document.querySelectorAll('button[aria-label="Copy thread reference"]');
      for (const btn of buttons) {
        const rect = btn.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          (btn as HTMLElement).click();
          return true;
        }
      }
      return false;
    });
    expect(clicked).toBe(true);

    // The filter panel must close.
    await expect(page.locator('.thread-filter-dropdown')).toHaveCount(0);

    // No toast should appear — the click was consumed. Polled assertion catches
    // a toast that briefly appears even if it auto-dismisses.
    await expect(page.locator('.toast')).toHaveCount(0, { timeout: 500 });
  });
});
