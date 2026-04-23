/**
 * Mobile thread content rendering tests.
 *
 * Reproduces the iOS Safari PWA bug where thread content sometimes
 * doesn't render after page reload or when switching threads.
 *
 * Root cause: sendMessage() cleared localStorage for new threads
 * (localStorage.removeItem('cognos-focused-thread')), so after reload
 * the focused thread ID was lost and compose view showed instead.
 * Fix: sendMessage() now persists the thread ID to localStorage.
 */
import { test, expect } from '@playwright/test';
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
} from './helpers';

test.describe('Mobile thread content rendering', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('thread content renders after reload on mobile without manual re-focus', async ({ page }) => {
    // 1. Create a thread with content
    await navigateToApp(page);
    const msg = uniqueMessage('mobile-reload');
    await sendMessage(page, `Say exactly: "persist ${msg}"`);
    await waitForResponse(page);

    // Verify content is visible
    await expect(userMessageBody(page)).toContainText(msg);

    // 2. Reload the page — focusedThreadId is restored from localStorage
    await page.reload();
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);

    // 3. Content MUST render without manually re-focusing from drawer.
    //    Root cause: sendMessage() clears localStorage for new threads, so
    //    after reload focusedThreadId is null and compose view shows instead.
    await expect(userMessageBody(page)).toContainText(msg, { timeout: 15_000 });

    // 4. Response should also be visible
    const response = page.locator('.response-content:visible').first();
    await expect(response).toBeVisible({ timeout: 10_000 });
    const text = await response.textContent();
    expect(text!.trim().length).toBeGreaterThan(0);
  });

  test('thread content renders when switching threads on mobile', async ({ page }) => {
    // 1. Create thread A with content
    await navigateToApp(page);
    const msgA = uniqueMessage('switch-A');
    await sendMessage(page, `Say exactly: "alpha ${msgA}"`);
    await waitForResponse(page);

    // Get thread A's nav element for later
    await openThreadDrawer(page);
    const threadANav = page.locator('[data-thread-nav]:visible').first();
    await expect(threadANav).toBeVisible({ timeout: 10_000 });
    const threadAId = await threadANav.getAttribute('data-thread-nav');

    // 2. Create thread B
    await newThread(page);
    const msgB = uniqueMessage('switch-B');
    await sendMessage(page, `Say exactly: "bravo ${msgB}"`);
    await waitForResponse(page);
    await expect(userMessageBody(page)).toContainText(msgB);

    // 3. Switch back to thread A via drawer
    await openThreadDrawer(page);
    const threadA = page.locator(`[data-thread-nav="${threadAId}"]:visible`).first();
    await expect(threadA).toBeVisible({ timeout: 10_000 });
    await threadA.click();
    await ensureOnThreadPane(page);

    // 4. Thread A's content MUST render — this is the core assertion.
    //    On iOS Safari PWA, loadThreadEvents may take time or fail,
    //    leaving the thread content blank.
    await expect(userMessageBody(page)).toContainText(msgA, { timeout: 15_000 });

    // 5. Response should also be visible
    const response = page.locator('.response-content:visible').first();
    await expect(response).toBeVisible({ timeout: 10_000 });
  });

  test('thread content area has non-zero height after thread switch on mobile', async ({ page }) => {
    // This catches the CSS bug where position:absolute;inset:0 fails
    // inside scroll-snap containers on iOS Safari.
    await navigateToApp(page);

    const msgA = uniqueMessage('height-A');
    await sendMessage(page, `Say exactly: "height ${msgA}"`);
    await waitForResponse(page);

    // Get thread A's ID
    await openThreadDrawer(page);
    const threadANav = page.locator('[data-thread-nav]:visible').first();
    const threadAId = await threadANav.getAttribute('data-thread-nav');

    // Create thread B to force a thread switch
    await newThread(page);
    const msgB = uniqueMessage('height-B');
    await sendMessage(page, `Say exactly: "height ${msgB}"`);
    await waitForResponse(page);

    // Switch back to thread A
    await openThreadDrawer(page);
    await page.locator(`[data-thread-nav="${threadAId}"]:visible`).first().click();
    await ensureOnThreadPane(page);

    // Wait for content to load
    await expect(userMessageBody(page)).toContainText(msgA, { timeout: 15_000 });

    // Verify .thread-content has non-zero height (catches CSS layout bugs)
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
    // Complex scenario: create two threads, switch between them, then reload.
    // This tests the combination of thread-switch + reload which can expose
    // stale state issues on iOS Safari PWA.
    await navigateToApp(page);

    // Create thread A
    const msgA = uniqueMessage('combo-A');
    await sendMessage(page, `Say exactly: "combo ${msgA}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);
    const threadANav = page.locator('[data-thread-nav]:visible').first();
    const threadAId = await threadANav.getAttribute('data-thread-nav');

    // Create thread B
    await newThread(page);
    const msgB = uniqueMessage('combo-B');
    await sendMessage(page, `Say exactly: "combo ${msgB}"`);
    await waitForResponse(page);

    // Switch to thread A
    await openThreadDrawer(page);
    await page.locator(`[data-thread-nav="${threadAId}"]:visible`).first().click();
    await ensureOnThreadPane(page);
    await expect(userMessageBody(page)).toContainText(msgA, { timeout: 15_000 });

    // Reload with thread A focused
    await page.reload();
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);

    // Thread A's content must still render after reload
    await expect(userMessageBody(page)).toContainText(msgA, { timeout: 15_000 });
  });
});
