import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer, ensureOnThreadPane, countVisibleThreadRows, userMessageBody, waitForVisibleInput, REAL_THREAD_ROW } from './helpers';

test.describe('Thread management', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('create and switch between two threads', async ({ page }) => {
    await navigateToApp(page);

    const msg1 = uniqueMessage('thread-1');
    await sendMessage(page, `Say exactly: "first ${msg1}"`);
    await waitForResponse(page);
    await expect(userMessageBody(page)).toContainText(msg1);

    await newThread(page);

    const msg2 = uniqueMessage('thread-2');
    await sendMessage(page, `Say exactly: "second ${msg2}"`);
    await waitForResponse(page);
    await expect(userMessageBody(page)).toContainText(msg2);

    await openThreadDrawer(page);
    const count = await countVisibleThreadRows(page);
    expect(count).toBeGreaterThanOrEqual(2);

    let foundFirst = false;
    const visibleRows = page.locator(`${REAL_THREAD_ROW}:visible`);
    const visibleCount = await visibleRows.count();
    for (let i = 0; i < visibleCount; i++) {
      await openThreadDrawer(page);
      await visibleRows.nth(i).click();
      await ensureOnThreadPane(page);
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

    const threadId = await (await waitForVisibleInput(page)).getAttribute('data-thread-id');
    expect(threadId, 'focused thread id missing after send').toBeTruthy();

    await newThread(page);

    await expect(async () => {
      await openThreadDrawer(page);
      await page.locator(`[data-thread-nav="${threadId}"]:visible`).first().click();
      await ensureOnThreadPane(page);
      await expect(userMessageBody(page)).toContainText(msg, { timeout: 5_000 });
    }).toPass({ timeout: 30_000, intervals: [1_000, 2_000] });
  });
});
