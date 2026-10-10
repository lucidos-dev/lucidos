import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, ensureMobileView } from './helpers';
import { ensureHomeThread } from './db-helpers';

/** A phone reaches the home thread from a Home icon paired with the mark in the
 *  thread pane's header. The thread drawer's header has no Home icon, so the
 *  menu its mark opens keeps a Home row; the thread pane's menu drops it. */
test.describe('The home thread from the phone header', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // A spec before this one may have truncated the threads, home included.
    ensureHomeThread();
    await navigateToApp(page);
    await ensureMobileView(page, 'thread');
  });

  test('Home and the mark centre together, and Home opens it', async ({ page }) => {
    const home = page.locator('.mobile-thread-header .header-mark-pair .home-thread-btn');
    await expect(home).toBeVisible();

    const offset = await page.evaluate(() => {
      const row = document.querySelector('.mobile-thread-header .mobile-header-row')!.getBoundingClientRect();
      const pair = document.querySelector('.mobile-thread-header .header-mark-pair')!.getBoundingClientRect();
      return (pair.left + pair.right) / 2 - (row.left + row.right) / 2;
    });
    expect(Math.abs(offset), `the pair sits ${offset}px off the row middle`).toBeLessThan(2.5);

    await home.click();
    await expect(page.locator('.thread-title-menu:visible')).toHaveText('Home');
  });

  test('Home and the mark spread apart when the row has room, and touch when it has none', async ({ page }) => {
    // The app writes the scale itself once preferences load, so wait for that
    // write before ours, or ours is reverted under the measurement.
    await page.waitForFunction(() => localStorage.getItem('lucidos-ui-scale') !== null, undefined, { timeout: 10_000 });

    const measure = () => page.evaluate(() => {
      const header = document.querySelector('.mobile-thread-header')!;
      const box = (sel: string) => header.querySelector(sel)!.getBoundingClientRect();
      const probe = document.createElement('div');
      probe.style.width = 'var(--header-mark-pair-gap-max)';
      document.body.appendChild(probe);
      const max = probe.getBoundingClientRect().width;
      probe.remove();
      const row = box('.mobile-header-row');
      const pair = box('.header-mark-pair');
      const home = box('.home-thread-btn');
      const mark = box('.brand-mark-slot');
      return {
        max,
        gap: mark.left - home.right,
        offset: (pair.left + pair.right) / 2 - (row.left + row.right) / 2,
        backClearance: pair.left - box('button[aria-label="Previous thread"]').right,
        forwardClearance: box('button[aria-label="Next thread"]').left - pair.right,
      };
    });

    for (const [scale, rootPx, roomy] of [[100, 16, true], [200, 32, false]] as const) {
      await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`), scale);
      await expect
        .poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize))
        .toBe(`${rootPx}px`);
      const m = await measure();
      if (roomy) {
        expect(m.gap, `ui-scale ${scale}: gap ${m.gap.toFixed(1)} is not the max ${m.max.toFixed(1)}`).toBeCloseTo(m.max, 0);
      } else {
        expect(m.gap, `ui-scale ${scale}: gap ${m.gap.toFixed(1)} did not close up`).toBeCloseTo(0, 0);
      }
      expect(Math.abs(m.offset), `ui-scale ${scale}: the pair sits ${m.offset}px off the row middle`).toBeLessThan(2.5);
      expect(m.backClearance, `ui-scale ${scale}: the back chevron overlaps the pair`).toBeGreaterThan(0);
      expect(m.forwardClearance, `ui-scale ${scale}: the forward chevron overlaps the pair`).toBeGreaterThan(0);
    }
  });

  test('only the thread drawer\'s menu carries a Home row', async ({ page }) => {
    const homeRow = page.locator('.brand-menu .home-thread-btn');

    await page.locator('.mobile-thread-header [data-role="brand-menu-toggle"]').click();
    await expect(page.locator('.brand-menu')).toBeVisible();
    await expect(homeRow).toHaveCount(0);
    await page.keyboard.press('Escape');
    await expect(page.locator('.brand-menu')).toBeHidden();

    await ensureMobileView(page, 'threads');
    await page.locator('.mobile-threads-header [data-role="brand-menu-toggle"]').click();
    const first = page.locator('.brand-menu [role="menuitem"]').first();
    await expect(first).toHaveText('Home');
    await first.click();
    await expect(page.locator('.brand-menu')).toBeHidden();
    await expect(page.locator('.thread-title-menu:visible')).toHaveText('Home');
  });
});
