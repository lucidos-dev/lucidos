import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  uniqueMessage,
  waitForResponse,
  newThread,
  openThreadDrawer,
  ensureOnThreadPane,
  waitForVisibleInput,
  userMessageBody,
  focusedThreadId,
} from './helpers';
import { clearAllThreads } from './db-helpers';

test.describe('Mobile thread content rendering', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('thread content renders after reload on mobile without manual re-focus', async ({ page }) => {
    await navigateToApp(page);
    const msg = uniqueMessage('mobile-reload');
    await sendMessage(page, `Say exactly: "persist ${msg}"`);
    await waitForResponse(page);

    await expect(userMessageBody(page)).toContainText(msg);

    await page.reload();
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);

    await expect(userMessageBody(page)).toContainText(msg, { timeout: 15_000 });

    const response = page.locator('.response-content:visible').first();
    await expect(response).toBeVisible({ timeout: 10_000 });
    const text = await response.textContent();
    expect(text!.trim().length).toBeGreaterThan(0);
  });

  test('thread content renders when switching threads on mobile', async ({ page }) => {
    await navigateToApp(page);
    const msgA = uniqueMessage('switch-A');
    await sendMessage(page, `Say exactly: "alpha ${msgA}"`);
    await waitForResponse(page);

    const threadAId = await focusedThreadId(page);

    await newThread(page);
    const msgB = uniqueMessage('switch-B');
    await sendMessage(page, `Say exactly: "bravo ${msgB}"`);
    await waitForResponse(page);
    await expect(userMessageBody(page)).toContainText(msgB);

    await openThreadDrawer(page);
    const threadA = page.locator(`[data-thread-nav="${threadAId}"]:visible`).first();
    await expect(threadA).toBeVisible({ timeout: 10_000 });
    await threadA.click();
    await ensureOnThreadPane(page);

    await expect(userMessageBody(page)).toContainText(msgA, { timeout: 15_000 });

    const response = page.locator('.response-content:visible').first();
    await expect(response).toBeVisible({ timeout: 10_000 });
  });

  test('thread content area has non-zero height after thread switch on mobile', async ({ page }) => {
    await navigateToApp(page);

    const msgA = uniqueMessage('height-A');
    await sendMessage(page, `Say exactly: "height ${msgA}"`);
    await waitForResponse(page);

    const threadAId = await focusedThreadId(page);

    await newThread(page);
    const msgB = uniqueMessage('height-B');
    await sendMessage(page, `Say exactly: "height ${msgB}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);
    await page.locator(`[data-thread-nav="${threadAId}"]:visible`).first().click();
    await ensureOnThreadPane(page);

    await expect(userMessageBody(page)).toContainText(msgA, { timeout: 15_000 });

    const contentHeight = await page.evaluate(() => {
      const els = document.querySelectorAll('.thread-content');
      for (const el of els) {
        const rect = el.getBoundingClientRect();
        if (rect.width > 0) return rect.height;
      }
      return 0;
    });
    expect(contentHeight).toBeGreaterThan(100);
  });

  test('thread content renders after reload with thread-switch-then-reload', async ({ page }) => {
    await navigateToApp(page);

    const msgA = uniqueMessage('combo-A');
    await sendMessage(page, `Say exactly: "combo ${msgA}"`);
    await waitForResponse(page);

    const threadAId = await focusedThreadId(page);

    await newThread(page);
    const msgB = uniqueMessage('combo-B');
    await sendMessage(page, `Say exactly: "combo ${msgB}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);
    await page.locator(`[data-thread-nav="${threadAId}"]:visible`).first().click();
    await ensureOnThreadPane(page);
    await expect(userMessageBody(page)).toContainText(msgA, { timeout: 15_000 });

    await page.reload();
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);

    await expect(userMessageBody(page)).toContainText(msgA, { timeout: 15_000 });
  });
});
