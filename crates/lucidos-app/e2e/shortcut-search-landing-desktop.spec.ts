import type { Locator, Page } from '@playwright/test';
import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';

async function pickShortcut(page: Page, label: string, how: 'click' | 'enter') {
  await page.keyboard.press('ControlOrMeta+k');
  const input = page.locator('.search-everywhere-input');
  await expect(input).toBeVisible();
  await input.fill(label);
  const hit = page.locator('.search-everywhere-result', { hasText: `${label} (` }).first();
  await expect(hit).toBeVisible();
  if (how === 'click') {
    await hit.click();
  } else {
    for (let i = 0; i < 20 && !(await hit.getAttribute('class'))?.includes('selected'); i++) {
      await page.keyboard.press('ArrowDown');
    }
    await page.keyboard.press('Enter');
  }
}

async function expectLanded(row: Locator) {
  await expect(row).toHaveClass(/nav-focus-stuck/, { timeout: 10_000 });
  await expect.poll(() => row.evaluate((el) => {
    const scroller = el.closest('.content-pane-body');
    if (!scroller) return 'no scroller';
    const r = el.getBoundingClientRect();
    const s = scroller.getBoundingClientRect();
    return r.top >= s.top && r.bottom <= s.bottom ? 'in view' : `out of view: row ${r.top}-${r.bottom}, pane ${s.top}-${s.bottom}`;
  }), { timeout: 5_000 }).toBe('in view');
  await row.page().waitForTimeout(1_500);
  await expect(row).toBeInViewport({ ratio: 1 });
  const widths = await row.evaluate((el) => ({
    row: el.getBoundingClientRect().width,
    list: el.parentElement!.getBoundingClientRect().width,
  }));
  expect(widths.row).toBe(widths.list);
}

test.describe('Search Everywhere → a shortcut row', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('scrolls to the row and highlights all of it', async ({ page }) => {
    await navigateToApp(page);
    await pickShortcut(page, 'Reset zoom', 'click');
    await expectLanded(page.locator('[data-search-anchor="shortcut:zoomReset"]'));
  });

  test('beats the scroll position an earlier visit saved', async ({ page }) => {
    await navigateToApp(page);
    await pickShortcut(page, 'Reset zoom', 'enter');
    await expectLanded(page.locator('[data-search-anchor="shortcut:zoomReset"]'));

    await page.keyboard.press('ControlOrMeta+,');
    await expect(page.locator('[data-search-anchor="shortcut:newThread"]')).toHaveCount(0);
    await pickShortcut(page, 'Search files', 'enter');
    await expectLanded(page.locator('[data-search-anchor="shortcut:searchFiles"]'));
  });
});
