import { test, expect } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, openThreadDrawer, ensureOnThreadPane, waitForVisibleInput, isMobileViewport } from './helpers';

test.describe('Thread pinning', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('pin a thread and verify pin indicator', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('pin-test');
    await sendMessage(page, `Say exactly: "pinned ${msg}"`);
    await waitForResponse(page);

    // Open drawer and find the thread
    await openThreadDrawer(page);
    const threadNav = page.locator('[data-thread-nav]:visible').first();
    await expect(threadNav).toBeVisible({ timeout: 15_000 });

    // Click the pin button
    const pinBtn = threadNav.locator('button[aria-label="Pin thread"]');
    await pinBtn.click();

    // After pinning, the button label should change to "Unpin thread"
    const unpinBtn = threadNav.locator('button[aria-label="Unpin thread"]');
    await expect(unpinBtn).toBeVisible({ timeout: 5_000 });
  });

  test('pinned thread persists after page reload', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('pin-reload');
    await sendMessage(page, `Say exactly: "persist-pin ${msg}"`);
    await waitForResponse(page);

    // Open drawer, find thread, pin it
    await openThreadDrawer(page);
    const threadNav = page.locator('[data-thread-nav]:visible').first();
    await expect(threadNav).toBeVisible({ timeout: 15_000 });
    const threadId = await threadNav.getAttribute('data-thread-nav');

    await threadNav.locator('button[aria-label="Pin thread"]').click();
    await expect(threadNav.locator('button[aria-label="Unpin thread"]')).toBeVisible({ timeout: 5_000 });

    // Reload the page
    await page.reload();
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);

    // Open drawer and verify it's still pinned
    await openThreadDrawer(page);

    // Wait for thread rows to load
    await expect(page.locator('[data-thread-nav]:visible').first()).toBeVisible({ timeout: 15_000 });

    const reloadedThread = page.locator(`[data-thread-nav="${threadId}"]:visible`).first();
    await expect(reloadedThread).toBeVisible({ timeout: 10_000 });
    await expect(reloadedThread.locator('button[aria-label="Unpin thread"]')).toBeVisible({ timeout: 5_000 });
  });

  test('unpin a thread', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('unpin-test');
    await sendMessage(page, `Say exactly: "unpin ${msg}"`);
    await waitForResponse(page);

    // Open drawer, pin it, then unpin it
    await openThreadDrawer(page);
    const threadNav = page.locator('[data-thread-nav]:visible').first();
    await expect(threadNav).toBeVisible({ timeout: 15_000 });

    await threadNav.locator('button[aria-label="Pin thread"]').click();
    await expect(threadNav.locator('button[aria-label="Unpin thread"]')).toBeVisible({ timeout: 5_000 });

    await threadNav.locator('button[aria-label="Unpin thread"]').click();
    await expect(threadNav.locator('button[aria-label="Pin thread"]')).toBeVisible({ timeout: 5_000 });
  });

  /* Regression: the pin button in the mobile thread title row sits at the
     left edge, which the absolutely-positioned `.edge-swipe-left` overlay
     covers (z-index: 1, leftmost 2.5rem). The title row itself has z-index
     2, but it lives inside `.thread-content` which creates a stacking
     context (transform: translateZ(0)) with no explicit z-index — so the
     title row's z-index is trapped inside `.thread-content` and the entire
     content paints at z-index 0 in the parent context, below the swipe
     zone. iOS Safari (and Chromium mobile emulation) hit-test the swipe
     zone instead of the pin button.

     Verifying via elementFromPoint: a tap at the pin button's center must
     resolve to the button (or its inner SVG), not the swipe overlay. */
  test('mobile: title bar pin button receives taps (not blocked by edge-swipe-zone)', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'mobile only');
    await navigateToApp(page);

    const msg = uniqueMessage('mobile-pin-tap');
    await sendMessage(page, `Say exactly: "title-pin ${msg}"`);
    await waitForResponse(page);

    await ensureOnThreadPane(page);

    // The title row is sticky at top of .thread-content
    const pinBtn = page.locator('.mobile-thread-title-row button[aria-label="Pin thread"]:visible').first();
    await expect(pinBtn).toBeVisible({ timeout: 10_000 });

    // Verify the topmost element at the pin button's visual center is
    // actually the button (or its child SVG/path), NOT the edge-swipe-zone.
    const result = await pinBtn.evaluate((btn) => {
      const r = btn.getBoundingClientRect();
      const cx = r.left + r.width / 2;
      const cy = r.top + r.height / 2;
      const hit = document.elementFromPoint(cx, cy);
      const hitClass = hit ? (hit as HTMLElement).className || '' : '';
      const hitsButton = !!hit && (hit === btn || btn.contains(hit));
      return { hitsButton, hitClass: typeof hitClass === 'string' ? hitClass : String(hitClass) };
    });
    expect(result.hitsButton, `pin button blocked by overlay: ${result.hitClass}`).toBe(true);

    // Sanity: tapping it actually toggles the pin state
    await pinBtn.tap();
    await expect(
      page.locator('.mobile-thread-title-row button[aria-label="Unpin thread"]:visible').first()
    ).toBeVisible({ timeout: 5_000 });
  });
});
