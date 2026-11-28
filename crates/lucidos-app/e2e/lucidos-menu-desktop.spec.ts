import { test, expect, Page } from './fixtures';
import { navigateToApp, assertHealthy, hideDashboardRail } from './helpers';

async function assertReachableInRow(page: Page, actionClass: string): Promise<void> {
  if (await page.locator(`.desktop-header ${actionClass}`).count() > 0) return;
  const more = page.locator('.desktop-header .thread-header-more');
  await expect(more, `${actionClass} is neither in the row nor folded into a ⋯ menu`).toHaveCount(1);
  await more.click();
  await expect(page.locator(`.thread-overflow-item${actionClass}`)).toHaveCount(1);
  await page.keyboard.press('Escape');
}

test.describe('Lucidos menu from the desktop mark', () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await hideDashboardRail(page);
  });

  test('opens with every control it owes, names the workspace, and closes on a second click', async ({ page }) => {
    await navigateToApp(page);

    const toggle = page.locator('.desktop-header [data-role="brand-menu-toggle"]');
    const menu = page.locator('.brand-menu');

    await expect(toggle).toHaveAttribute('data-conn', /connected|connecting|disconnected/);
    await expect(toggle).toHaveAttribute('aria-label', /^Lucidos menu · (.+ · )*connect/);

    await assertReachableInRow(page, '.brand-compose-btn');
    await assertReachableInRow(page, '.search-everywhere-btn');

    await toggle.click();
    await expect(menu).toBeVisible();

    for (const label of ['Workspaces', 'Refresh']) {
      await expect(menu.locator('.brand-menu-item', { hasText: label })).toHaveCount(1);
    }

    for (const label of ['New thread', 'Search everywhere', 'Setup guide']) {
      await expect(menu.locator('.brand-menu-item', { hasText: label })).toHaveCount(0);
    }

    const workspaces = menu.locator('.brand-menu-item', { hasText: 'Workspaces' });
    await expect(workspaces.locator('.brand-menu-value-name')).toHaveCount(1);
    await expect(workspaces.locator('.brand-menu-value-name')).not.toBeEmpty();

    const spelled = await workspaces.locator('.brand-menu-value-name').evaluate((el: HTMLElement) => {
      const original = el.textContent;
      el.textContent = 'development';
      const fits = el.clientWidth > 0 && el.scrollWidth - el.clientWidth <= 1;
      el.textContent = original;
      return fits;
    });
    expect(spelled, 'the Workspaces pill ellipsises a name as ordinary as "development"').toBe(true);

    const hasGateway = await page.locator('meta[name="lucidos-gateway-port"]').count() > 0;
    if (hasGateway) {
      await expect(workspaces).toHaveJSProperty('tagName', 'A');
      await expect(workspaces).toHaveAttribute('href', /\/~\/\?pick$/);
    } else {
      await expect(workspaces).not.toHaveJSProperty('tagName', 'A');
      await expect(workspaces).not.toHaveAttribute('href');
    }
    await expect(workspaces.locator('.brand-menu-value-chevron')).toHaveCount(0);
    await expect(workspaces.locator('.brand-menu-value-check')).toHaveCount(1);
    await expect(workspaces).toHaveCSS('text-decoration-line', 'none');

    const centres = await page.evaluate(() => {
      const mid = (el: Element): number => {
        const r = el.getBoundingClientRect();
        return (r.left + r.right) / 2;
      };
      return {
        panel: mid(document.querySelector('.brand-menu')!),
        pane: mid(document.querySelector('.desktop-header .pane-header-brand')!),
        window: window.innerWidth / 2,
      };
    });
    expect(centres.panel).toBeCloseTo(centres.pane, 0);
    expect(
      Math.abs(centres.panel - centres.window),
      'the panel landed on the window axis, so pane centring is not in force',
    ).toBeGreaterThan(1);

    await toggle.click();
    await expect(menu).toHaveCount(0);
  });

  test('says why the mark is dim, and stops saying it on reconnect', async ({ page }) => {
    test.slow();

    await navigateToApp(page);

    const toggle = page.locator('.desktop-header [data-role="brand-menu-toggle"]');
    const menu = page.locator('.brand-menu');
    const notice = page.locator('.brand-menu-notice');
    const open = async () => {
      await toggle.click();
      await expect(menu).toBeVisible();
    };
    const close = async () => {
      await toggle.click();
      await expect(menu).toHaveCount(0);
    };

    await expect(toggle).toHaveAttribute('data-conn', 'connected', { timeout: 30_000 });
    await open();
    await expect(notice).toHaveCount(0);
    await close();

    await page.route('**/api/v1/health', (route) => route.abort());
    await expect(toggle).toHaveAttribute('data-conn', 'disconnected', { timeout: 60_000 });

    await open();
    await expect(notice).toHaveCount(1);
    await expect(notice).toContainText(/^Cannot reach \S/);
    await expect(notice.locator('.status-dot.disconnected')).toHaveCount(1);
    await expect(notice.locator('button')).toHaveCount(0);
    await close();

    await page.unroute('**/api/v1/health');
    await expect(toggle).toHaveAttribute('data-conn', 'connected', { timeout: 60_000 });
    await open();
    await expect(notice).toHaveCount(0);
    await close();
  });

  test('opening and closing the menu does not move the mark or the workspace name', async ({ page }) => {
    await navigateToApp(page);

    const toggle = page.locator('.desktop-header [data-role="brand-menu-toggle"]');
    const menu = page.locator('.brand-menu');
    const name = page.locator('.desktop-header .workspace-name-label');
    await expect(name).toBeVisible();

    const measure = () => page.evaluate(() => {
      const box = (sel: string) => {
        const el = document.querySelector(sel);
        if (!el) return null;
        const r = el.getBoundingClientRect();
        return { x: r.left, y: r.top, w: r.width, h: r.height };
      };
      return {
        mark: box('.desktop-header [data-role="brand-menu-toggle"]'),
        name: box('.desktop-header .workspace-name-label'),
      };
    });

    for (const overlay of [false, true]) {
      await page.evaluate((on) => {
        const root = document.documentElement;
        if (on) {
          root.setAttribute('data-titlebar-overlay', '');
          root.style.setProperty('--titlebar-inset', '28px');
        } else {
          root.removeAttribute('data-titlebar-overlay');
          root.style.removeProperty('--titlebar-inset');
        }
      }, overlay);
      await page.waitForTimeout(500);

      const build = overlay ? 'packaged macOS' : 'web';
      const before = await measure();
      await toggle.click();
      await expect(menu).toBeVisible();
      const open = await measure();
      await toggle.click();
      await expect(menu).toHaveCount(0);
      const after = await measure();

      for (const part of ['mark', 'name'] as const) {
        for (const axis of ['x', 'y', 'w', 'h'] as const) {
          expect(
            Math.abs(open[part]![axis] - before[part]![axis]),
            `${build}: the ${part}'s ${axis} moved while the menu was open`,
          ).toBeLessThan(0.5);
          expect(
            Math.abs(after[part]![axis] - before[part]![axis]),
            `${build}: the ${part}'s ${axis} did not return after closing`,
          ).toBeLessThan(0.5);
        }
      }
    }
  });

  test('the brand label never overflows the region that clips it, at any mark size', async ({ page }) => {
    await navigateToApp(page);

    const toggle = page.locator('.desktop-header [data-role="brand-menu-toggle"]');
    const menu = page.locator('.brand-menu');
    await expect(page.locator('.desktop-header .pane-header-brand')).toBeVisible();
    await expect(page.locator('.desktop-header .pane-header-brand-label')).toBeVisible();

    const fits = () => page.evaluate(() => {
      const region = document.querySelector('.desktop-header .pane-header-brand') as HTMLElement;
      const label = document.querySelector('.desktop-header .pane-header-brand-label') as HTMLElement;
      const r = region.getBoundingClientRect();
      const l = label.getBoundingClientRect();
      return {
        above: r.top - l.top,
        below: l.bottom - r.bottom,
        scrollable: region.scrollHeight - region.clientHeight,
      };
    });

    for (const tap of [null, '2.6rem']) {
      await page.evaluate((value) => {
        const root = document.documentElement;
        if (value) root.style.setProperty('--header-mark-tap', value);
        else root.style.removeProperty('--header-mark-tap');
      }, tap);
      await page.waitForTimeout(100);

      const at = tap ? `--header-mark-tap: ${tap}` : 'the shipped mark';
      for (const state of ['closed', 'open'] as const) {
        if (state === 'open') {
          await toggle.click();
          await expect(menu).toBeVisible();
        }
        const box = await fits();
        expect(box.above, `${at} (${state}): the label hangs above the region`).toBeLessThan(0.5);
        expect(box.below, `${at} (${state}): the label hangs below the region`).toBeLessThan(0.5);
        expect(box.scrollable, `${at} (${state}): the region has scrollable overflow`)
          .toBeLessThanOrEqual(1);
        if (state === 'open') {
          await toggle.click();
          await expect(menu).toHaveCount(0);
        }
      }
    }
  });
});
