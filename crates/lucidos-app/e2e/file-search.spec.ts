import { test, expect, Page } from '@playwright/test';
import { assertHealthy, waitForVisibleInput, isMobileViewport, blurActiveElement, ensureOnThreadPane, openFilesPanel, gotoWithRetry } from './helpers';

/** Open file search — tap on mobile, click on desktop. Waits for the
 *  signal-driven Preact render to apply the open state before returning,
 *  so callers can immediately assert on overlay/input visibility. */
async function openFileSearch(page: Page): Promise<void> {
  await blurActiveElement(page);
  await page.waitForTimeout(100);

  const btn = page.locator('.file-search-btn:visible').first();
  await expect(btn).toBeVisible({ timeout: 5_000 });
  if (isMobileViewport(page)) {
    await btn.tap();
  } else {
    await btn.click();
  }
  await page.waitForFunction(() => {
    const overlay = document.querySelector('.file-search-overlay');
    return overlay !== null && !overlay.classList.contains('file-search-closed');
  }, undefined, { timeout: 3_000 });
}

async function isSearchOpen(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const overlay = document.querySelector('.file-search-overlay');
    if (!overlay) return false;
    return !overlay.classList.contains('file-search-closed');
  });
}

test.describe('File search', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // gotoWithRetry: a bare page.goto can hang the whole 120s test budget on
    // mobile-webkit when the app-root navigation wedges (see gotoWithRetry).
    // Hydration is checked explicitly below — the #app check is what guarantees
    // the SPA is ready.
    await gotoWithRetry(page, '/');
    await page.waitForFunction(() =>
      document.querySelector('#app')?.childElementCount! > 0,
      undefined, { timeout: 30_000 },
    );
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);
  });

  test('search opens with focused input', async ({ page }) => {
    await openFilesPanel(page);
    await openFileSearch(page);

    expect(await isSearchOpen(page)).toBe(true);

    // Input should be focused (triggers keyboard on real iOS)
    await page.waitForFunction(() => {
      const input = document.querySelector('[data-role="file-search-input"]');
      return input !== null && document.activeElement === input;
    }, undefined, { timeout: 3_000 });
  });

  test('close button works on first tap/click', async ({ page }) => {
    await openFilesPanel(page);
    await openFileSearch(page);
    expect(await isSearchOpen(page)).toBe(true);

    // Wait for input to be focused (simulates keyboard-open state)
    await page.waitForFunction(() => {
      const input = document.querySelector('[data-role="file-search-input"]');
      return input !== null && document.activeElement === input;
    }, undefined, { timeout: 3_000 });

    // Close — tap on mobile, click on desktop
    const closeBtn = page.locator('.file-search-close:visible').first();
    await expect(closeBtn).toBeVisible({ timeout: 3_000 });
    if (isMobileViewport(page)) {
      await closeBtn.tap();
    } else {
      await closeBtn.click();
    }

    // Should close immediately (not require a second tap)
    await page.waitForFunction(() => {
      const overlay = document.querySelector('.file-search-overlay');
      return overlay?.classList.contains('file-search-closed');
    }, undefined, { timeout: 3_000 });
    expect(await isSearchOpen(page)).toBe(false);
  });

  test('backdrop dismisses search on first tap/click', async ({ page }) => {
    await openFilesPanel(page);
    await openFileSearch(page);
    expect(await isSearchOpen(page)).toBe(true);

    // Wait for input focus
    await page.waitForFunction(() => {
      const input = document.querySelector('[data-role="file-search-input"]');
      return input !== null && document.activeElement === input;
    }, undefined, { timeout: 3_000 });

    // Tap/click the backdrop (bottom of screen, outside the modal)
    const vp = page.viewportSize()!;
    if (isMobileViewport(page)) {
      await page.touchscreen.tap(vp.width / 2, vp.height - 20);
    } else {
      await page.mouse.click(vp.width / 2, vp.height - 20);
    }

    await page.waitForFunction(() => {
      const overlay = document.querySelector('.file-search-overlay');
      return overlay?.classList.contains('file-search-closed');
    }, undefined, { timeout: 3_000 });
    expect(await isSearchOpen(page)).toBe(false);
  });

  test('search panel is pinned to top', async ({ page }) => {
    await openFilesPanel(page);
    await openFileSearch(page);

    // Modal should be near the top (below safe area + padding), not centered
    const modalTop = await page.evaluate(() => {
      const modal = document.querySelector('.file-search-modal');
      return modal ? modal.getBoundingClientRect().top : -1;
    });
    expect(modalTop).toBeGreaterThan(0);
    expect(modalTop).toBeLessThan(150);
  });
});
