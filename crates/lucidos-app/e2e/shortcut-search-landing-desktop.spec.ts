/**
 * Picking a shortcut in Search Everywhere lands on its row in Keyboard
 * Shortcuts: the row scrolls into view and wears the navigation focus marker
 * across its whole width, like every other settings landing.
 *
 * The unit test (`shortcut-search-landing.test.tsx`) only proves the row
 * carries the anchor. This drives the real palette, so it also covers the
 * scroll and the marker.
 */
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
    // Walk the keyboard cursor down to the hit, then take it with Enter.
    for (let i = 0; i < 20 && !(await hit.getAttribute('class'))?.includes('selected'); i++) {
      await page.keyboard.press('ArrowDown');
    }
    await page.keyboard.press('Enter');
  }
}

async function expectLanded(row: Locator) {
  await expect(row).toHaveClass(/nav-focus-stuck/, { timeout: 10_000 });
  // Polled, since the landing scrolls smoothly.
  await expect.poll(() => row.evaluate((el) => {
    const scroller = el.closest('.content-pane-body');
    if (!scroller) return 'no scroller';
    const r = el.getBoundingClientRect();
    const s = scroller.getBoundingClientRect();
    return r.top >= s.top && r.bottom <= s.bottom ? 'in view' : `out of view: row ${r.top}-${r.bottom}, pane ${s.top}-${s.bottom}`;
  }), { timeout: 5_000 }).toBe('in view');
  // Still there once anything that places the reader after a view change has
  // had its turn.
  await row.page().waitForTimeout(1_500);
  await expect(row).toBeInViewport({ ratio: 1 });
  // The marker sits on the row itself, so it washes the full row width.
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

  // The scroll memory keeps a position per content view. A landing must beat
  // the one an earlier visit to Keyboard Shortcuts left behind.
  test('beats the scroll position an earlier visit saved', async ({ page }) => {
    await navigateToApp(page);
    await pickShortcut(page, 'Reset zoom', 'enter');
    await expectLanded(page.locator('[data-search-anchor="shortcut:zoomReset"]'));

    // Leave for another settings page, then land near the top of the list.
    await page.keyboard.press('ControlOrMeta+,');
    await expect(page.locator('[data-search-anchor="shortcut:newThread"]')).toHaveCount(0);
    await pickShortcut(page, 'Search files', 'enter');
    await expectLanded(page.locator('[data-search-anchor="shortcut:searchFiles"]'));
  });
});
