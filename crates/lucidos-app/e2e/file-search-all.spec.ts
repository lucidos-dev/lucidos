import { test, expect, Page } from './fixtures';
import { assertHealthy, waitForVisibleInput, isMobileViewport, blurActiveElement, ensureOnThreadPane, openFilesPanel, gotoWithRetry, clickHeaderAction } from './helpers';

async function openFileSearch(page: Page): Promise<void> {
  await blurActiveElement(page);
  await page.waitForTimeout(100);

  await clickHeaderAction(page, '.file-search-btn');
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

    await page.waitForFunction(() => {
      const input = document.querySelector('[data-role="file-search-input"]');
      return input !== null && document.activeElement === input;
    }, undefined, { timeout: 3_000 });
  });

  test('close button works on first tap/click', async ({ page }) => {
    await openFilesPanel(page);
    await openFileSearch(page);
    expect(await isSearchOpen(page)).toBe(true);

    await page.waitForFunction(() => {
      const input = document.querySelector('[data-role="file-search-input"]');
      return input !== null && document.activeElement === input;
    }, undefined, { timeout: 3_000 });

    const closeBtn = page.locator('.file-search-close:visible').first();
    await expect(closeBtn).toBeVisible({ timeout: 3_000 });
    if (isMobileViewport(page)) {
      await closeBtn.tap();
    } else {
      await closeBtn.click();
    }

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

    await page.waitForFunction(() => {
      const input = document.querySelector('[data-role="file-search-input"]');
      return input !== null && document.activeElement === input;
    }, undefined, { timeout: 3_000 });

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

    const modalOffset = await page.evaluate(() => {
      const modal = document.querySelector('.file-search-modal');
      const header = document.querySelector('.app-header');
      if (!modal) return -1;
      return modal.getBoundingClientRect().top - (header?.getBoundingClientRect().bottom ?? 0);
    });
    expect(modalOffset).toBeGreaterThan(0);
    expect(modalOffset).toBeLessThan(60);
  });
});
