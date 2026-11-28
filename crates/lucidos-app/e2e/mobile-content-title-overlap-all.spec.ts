import { test, expect, Page } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { assertHealthy, gotoWithRetry, ensureMobileView, navigateToApp, clickVisibleElement, openFilesPanel } from './helpers';

const APP_ID = 'e2e-mobile-title-overlap';
const APP_NAME = 'Half Marathon';
let fixture: { cleanup: () => void };

interface ContentMetrics {
  titleLeft: number;
  titleRight: number;
  titleWidth: number;
  navRight: number;
  actionsLeft: number;
  hasOverflow: boolean;
  bareActions: number;
  trailingIcons: number;
  textCenter: number;
  clusterCenter: number;
  rowCenter: number;
}

async function measureContent(page: Page): Promise<ContentMetrics | null> {
  return page.evaluate(() => {
    const header = document.querySelector('.mobile-content-header') as HTMLElement | null;
    if (!header || header.getBoundingClientRect().width === 0) return null;
    const title = header.querySelector('.mobile-content-title') as HTMLElement | null;
    const nav = header.querySelector('.hamburger-panel') as HTMLElement | null;
    const actions = header.querySelector('.content-header-actions') as HTMLElement | null;
    const cluster = header.querySelector('.header-title-cluster') as HTMLElement | null;
    const row = header.querySelector('.mobile-header-row') as HTMLElement | null;
    if (!title || !nav || !actions || !cluster || !row) return null;
    const t = title.getBoundingClientRect();
    const a = actions.getBoundingClientRect();
    const c = cluster.getBoundingClientRect();
    const r = row.getBoundingClientRect();
    if (t.width === 0 || a.width === 0) return null;
    const trailingIcons = actions.querySelectorAll('.icon-btn.header-icon').length;
    const bells = actions.querySelectorAll('.notifications-bell').length;
    const more = actions.querySelectorAll('.content-header-more').length;
    return {
      titleLeft: t.left,
      titleRight: t.right,
      titleWidth: t.width,
      textCenter: t.left + t.width / 2,
      clusterCenter: c.left + c.width / 2,
      rowCenter: r.left + r.width / 2,
      navRight: nav.getBoundingClientRect().right,
      actionsLeft: a.left,
      hasOverflow: more > 0,
      bareActions: trailingIcons - bells - more,
      trailingIcons,
    };
  });
}

async function setScaleAndSettle(page: Page, scale: number): Promise<ContentMetrics> {
  await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`), scale);
  await expect
    .poll(async () => {
      const m = await measureContent(page);
      return !!m && m.titleWidth > 0
        && m.titleRight <= m.actionsLeft + 0.6
        && m.titleLeft >= m.navRight - 0.6;
    }, { timeout: 6_000, message: `content title never settled clear of the clusters at ui-scale ${scale}` })
    .toBe(true);
  const measured = (await measureContent(page))!;
  console.log(JSON.stringify({ scale, phoneTitleWidth: measured.titleWidth }));
  return measured;
}

function expectClearAndCentred(m: ContentMetrics, scale: number): void {
  expect(
    m.titleRight,
    `ui-scale ${scale}: title right ${m.titleRight.toFixed(1)} paints under actions left ${m.actionsLeft.toFixed(1)}`,
  ).toBeLessThanOrEqual(m.actionsLeft + 0.6);
  expect(
    m.titleLeft,
    `ui-scale ${scale}: title left ${m.titleLeft.toFixed(1)} paints under nav right ${m.navRight.toFixed(1)}`,
  ).toBeGreaterThanOrEqual(m.navRight - 0.6);
  expect(m.titleWidth, `ui-scale ${scale}: title has zero width (hidden)`).toBeGreaterThan(0);
  expect(Math.abs(m.textCenter - m.rowCenter),
    `ui-scale ${scale}: content title is off the pane axis`).toBeLessThan(1.5);

}

test.describe('Mobile content header collapses to one icon and never overlaps', () => {
  test.use({ viewport: { width: 393, height: 852 } });

  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: APP_NAME, description: 'e2e fixture' },
      html: `<!DOCTYPE html><html><head><meta charset="UTF-8"><title>${APP_NAME}</title></head><body><div id="ready">ready</div></body></html>`,
      js: '',
    });
  });

  test.afterAll(() => {
    fixture.cleanup();
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a view WITH context actions shows only the overflow trigger and the bell', async ({ page }) => {
    await page.addInitScript((id) => localStorage.setItem('app-window-open', id), APP_ID);
    await gotoWithRetry(page, '/');
    await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible({ timeout: 15_000 });
    await ensureMobileView(page, 'content');
    await expect(page.locator('.mobile-content-header .mobile-content-title')).toBeVisible({ timeout: 10_000 });

    for (const scale of [100, 125, 150, 175]) {
      const m = await setScaleAndSettle(page, scale);
      expect(m.hasOverflow, `ui-scale ${scale}: the actions should always be behind the overflow trigger`).toBe(true);
      expect(
        m.bareActions,
        `ui-scale ${scale}: ${m.bareActions} context action(s) rendered as header buttons instead of collapsing`,
      ).toBe(0);
      expect(m.trailingIcons, `ui-scale ${scale}: the trailing cluster should be exactly the overflow trigger + bell`).toBe(2);
      expectClearAndCentred(m, scale);
    }
  });

  test('a view with ONE context action shows that action, not the overflow trigger', async ({ page }) => {
    await navigateToApp(page);
    await openFilesPanel(page);
    await ensureMobileView(page, 'content');
    await expect(page.locator('.mobile-content-header .mobile-content-title'))
      .toHaveText('Files', { timeout: 15_000 });

    for (const scale of [100, 125, 150, 175]) {
      const m = await setScaleAndSettle(page, scale);
      expect(m.hasOverflow, `ui-scale ${scale}: a lone action needs no overflow trigger`).toBe(false);
      expect(m.bareActions, `ui-scale ${scale}: the one action should ride the row`).toBe(1);
      expect(m.trailingIcons, `ui-scale ${scale}: the trailing cluster should be the action + bell`).toBe(2);
      expectClearAndCentred(m, scale);
    }

    await expect(page.locator('.mobile-content-header .file-search-btn.icon-btn')).toBeVisible();
  });

  test('a view with NO context actions shows the bell alone', async ({ page }) => {
    await navigateToApp(page);

    await clickVisibleElement(page, '.hamburger-panel');
    await expect
      .poll(() => clickVisibleElement(page, '.drawer-item', 'Settings'), { timeout: 10_000 })
      .toBe(true);
    await ensureMobileView(page, 'content');
    await expect
      .poll(() => clickVisibleElement(page, '.settings-nav-row', 'Appearance & Behavior'), { timeout: 10_000 })
      .toBe(true);

    const title = page.locator('.mobile-content-header .mobile-content-title');
    await expect(title).toHaveText('Appearance', { timeout: 15_000 });
    await expect(title).toHaveAttribute('data-tooltip', 'Appearance & Behavior');

    for (const scale of [100, 125, 150, 175]) {
      const m = await setScaleAndSettle(page, scale);
      expect(m.bareActions, `ui-scale ${scale}: settings should carry no context actions`).toBe(0);
      expect(m.hasOverflow, `ui-scale ${scale}: nothing to collapse, so no overflow trigger`).toBe(false);
      expect(m.trailingIcons, `ui-scale ${scale}: the trailing cluster should be the bell alone`).toBe(1);
      expectClearAndCentred(m, scale);
    }
  });

  test('the thread header keeps its cluster centred and clear of its edge controls', async ({ page }) => {
    await gotoWithRetry(page, '/');
    await ensureMobileView(page, 'thread');
    await page.evaluate(() => document.documentElement.style.setProperty('--user-ui-scale', '125%'));

    const brand = await page.evaluate(() => {
      const header = document.querySelector('.mobile-thread-header') as HTMLElement | null;
      const row = header?.querySelector('.mobile-header-row') as HTMLElement | null;
      const brandEl = header?.querySelector('.header-nav-cluster') as HTMLElement | null;
      const nav = header?.querySelector('.thread-toggle') as HTMLElement | null;
      if (!header || !row || !brandEl || !nav) return null;
      const b = brandEl.getBoundingClientRect();
      const r = row.getBoundingClientRect();
      return {
        center: b.left + b.width / 2,
        rowCenter: r.left + r.width / 2,
        left: b.left,
        navRight: nav.getBoundingClientRect().right,
      };
    });
    expect(brand, 'nav cluster / drawer toggle not found').not.toBeNull();
    expect(
      Math.abs(brand!.center - brand!.rowCenter),
      `brand center ${brand!.center.toFixed(1)} vs row center ${brand!.rowCenter.toFixed(1)}`,
    ).toBeLessThan(2.5);
    expect(
      brand!.left,
      `brand left ${brand!.left.toFixed(1)} overlaps nav right ${brand!.navRight.toFixed(1)}`,
    ).toBeGreaterThanOrEqual(brand!.navRight - 0.5);
  });
});
