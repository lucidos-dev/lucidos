import { test, expect } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer, ensureOnThreadPane, countVisibleThreadRows, userMessageBody, REAL_THREAD_ROW } from './helpers';

test.describe('Thread management', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('create and switch between two threads', async ({ page }) => {
    await navigateToApp(page);

    // Create first thread
    const msg1 = uniqueMessage('thread-1');
    await sendMessage(page, `Say exactly: "first ${msg1}"`);
    await waitForResponse(page);
    await expect(userMessageBody(page)).toContainText(msg1);

    // Start a new thread
    await newThread(page);

    // Create second thread
    const msg2 = uniqueMessage('thread-2');
    await sendMessage(page, `Say exactly: "second ${msg2}"`);
    await waitForResponse(page);
    await expect(userMessageBody(page)).toContainText(msg2);

    // Open drawer and switch back to first thread
    await openThreadDrawer(page);
    const count = await countVisibleThreadRows(page);
    expect(count).toBeGreaterThanOrEqual(2);

    // Click each visible REAL thread (skip compose-draft rows) to find the
    // one with our first message
    let foundFirst = false;
    const visibleRows = page.locator(`${REAL_THREAD_ROW}:visible`);
    const visibleCount = await visibleRows.count();
    for (let i = 0; i < visibleCount; i++) {
      await openThreadDrawer(page);
      await visibleRows.nth(i).click();
      await ensureOnThreadPane(page);
      // Wait for thread content to load after clicking
      await page.waitForFunction(() => {
        const els = document.querySelectorAll('.thread-content');
        return Array.from(els).some(el => {
          const rect = el.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').length > 0;
        });
      }, undefined, { timeout: 10_000 });
      const content = await page.locator('.thread-content:visible').first().textContent();
      if (content?.includes(msg1)) {
        foundFirst = true;
        break;
      }
    }
    expect(foundFirst).toBe(true);
  });

  test('thread loads with correct messages when clicked', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('thread-load');
    await sendMessage(page, `Say exactly: "loaded ${msg}"`);
    await waitForResponse(page);

    // Navigate away
    await newThread(page);

    // Open drawer and click on the most recent REAL thread (skip drafts)
    await openThreadDrawer(page);
    await page.locator(`${REAL_THREAD_ROW}:visible`).first().click();
    await ensureOnThreadPane(page);

    // Verify the thread content is still there
    await expect(userMessageBody(page)).toContainText(msg, { timeout: 10_000 });
  });
});
