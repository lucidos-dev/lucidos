import { test, expect, Page } from './fixtures';
import { navigateToApp, assertHealthy, ensureMobileView } from './helpers';
import { ensureHomeThread } from './db-helpers';

/** A phone reaches Home from a Home icon in the thread pane's header, left of
 *  the mark, with the dashboard toggle on its right. It lands on Home as a
 *  plain chat (ADR 0496, amended). The thread drawer's header has no Home icon,
 *  so the menu its mark opens keeps a Home row; the thread pane's menu drops it. */
test.describe('The home thread from the phone header', () => {
  let home: string;

  /** Home is focused, and it is a plain chat with no dashboard face. */
  async function expectHomeOpen(page: Page): Promise<void> {
    await expect
      .poll(() => page.evaluate(() => localStorage.getItem('lucidos-focused-thread')))
      .toBe(home);
    await expect(page.locator('.home-pane')).toHaveCount(0);
    await expect(page.locator('[data-home-face]')).toHaveCount(0);
  }

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // A spec before this one may have truncated the threads, home included.
    home = ensureHomeThread();
    await navigateToApp(page);
    await ensureMobileView(page, 'thread');
  });

  test('the mark sits on the row middle between Home and the dashboard, and Home opens it', async ({ page }) => {
    const home = page.locator('.mobile-thread-header .header-mark-group .home-thread-btn');
    await expect(home).toBeVisible();
    await expect(page.locator('.mobile-thread-header .header-mark-group .dashboard-sheet-toggle')).toBeVisible();

    const offset = await page.evaluate(() => {
      const row = document.querySelector('.mobile-thread-header .mobile-header-row')!.getBoundingClientRect();
      const mark = document.querySelector('.mobile-thread-header .header-mark-group .brand-mark-slot')!.getBoundingClientRect();
      return (mark.left + mark.right) / 2 - (row.left + row.right) / 2;
    });
    expect(Math.abs(offset), `the mark sits ${offset}px off the row middle`).toBeLessThan(1);

    await home.click();
    await expectHomeOpen(page);
  });

  /** Sets the scale and measures the header in ONE synchronous page task. The
   *  app re-applies the saved scale on every preferences load, and a refetch
   *  landing between a separate write and read reverts ours. Nothing can land
   *  inside one task, so a measurement taken at the wrong root size is retried. */
  async function measureAtScale(page: Page, scale: number, rootPx: number) {
    const handle = await page.waitForFunction(
      ([s, px]) => {
        const root = document.documentElement;
        root.style.setProperty('--user-ui-scale', `${s}%`);
        if (getComputedStyle(root).fontSize !== `${px}px`) return null;
        const header = document.querySelector('.mobile-thread-header')!;
        const box = (sel: string) => header.querySelector(sel)!.getBoundingClientRect();
        const probe = document.createElement('div');
        probe.style.width = 'var(--header-mark-group-gap-max)';
        document.body.appendChild(probe);
        const max = probe.getBoundingClientRect().width;
        probe.remove();
        const row = box('.mobile-header-row');
        const group = box('.header-mark-group');
        const home = box('.home-thread-btn');
        const mark = box('.brand-mark-slot');
        const dash = box('.dashboard-sheet-toggle');
        return {
          max,
          homeGap: mark.left - home.right,
          dashGap: dash.left - mark.right,
          offset: (mark.left + mark.right) / 2 - (row.left + row.right) / 2,
          backClearance: group.left - box('button[aria-label="Previous thread"]').right,
          forwardClearance: box('button[aria-label="Next thread"]').left - group.right,
        };
      },
      [scale, rootPx] as const,
      { timeout: 30_000 },
    );
    return handle.jsonValue();
  }

  async function expectGroupAtScale(page: Page, scale: number, rootPx: number, roomy: boolean): Promise<void> {
    const m = await measureAtScale(page, scale, rootPx);
    for (const [side, gap] of [['Home', m.homeGap], ['dashboard', m.dashGap]] as const) {
      if (roomy) {
        expect(gap, `ui-scale ${scale}: ${side} gap ${gap.toFixed(1)} is not the max ${m.max.toFixed(1)}`).toBeCloseTo(m.max, 0);
      } else {
        expect(gap, `ui-scale ${scale}: ${side} gap ${gap.toFixed(1)} did not close up`).toBeCloseTo(0, 0);
      }
    }
    expect(Math.abs(m.offset), `ui-scale ${scale}: the mark sits ${m.offset}px off the row middle`).toBeLessThan(1);
    expect(m.backClearance, `ui-scale ${scale}: the back chevron overlaps the group`).toBeGreaterThan(0);
    expect(m.forwardClearance, `ui-scale ${scale}: the forward chevron overlaps the group`).toBeGreaterThan(0);
  }

  test('the group spreads out when the row has room, and touches when it has none', async ({ page }) => {
    // The app writes the scale itself once preferences load, so wait for that
    // write before ours.
    await page.waitForFunction(() => localStorage.getItem('lucidos-ui-scale') !== null, undefined, { timeout: 10_000 });
    await expect(page.locator('.mobile-thread-header .header-mark-group .home-thread-btn')).toBeVisible();

    await expectGroupAtScale(page, 100, 16, true);
    await expectGroupAtScale(page, 200, 32, false);
  });

  test('a preferences apply that reverts the scale between tasks cannot skew the measurement', async ({ page }) => {
    await expect(page.locator('.mobile-thread-header .header-mark-group .home-thread-btn')).toBeVisible();
    // Stands in for the app's re-apply on a preferences refetch: it undoes any
    // scale other than 100% as soon as the writing task ends.
    await page.evaluate(() => {
      const root = document.documentElement;
      new MutationObserver(() => {
        if (root.style.getPropertyValue('--user-ui-scale') !== '100%') root.style.setProperty('--user-ui-scale', '100%');
      }).observe(root, { attributes: true, attributeFilter: ['style'] });
    });

    await expectGroupAtScale(page, 200, 32, false);
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
    await expectHomeOpen(page);
  });
});
