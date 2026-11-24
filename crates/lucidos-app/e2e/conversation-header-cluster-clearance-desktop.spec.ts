import { test, expect, type Page } from './fixtures';
import {
  assertHealthy, gotoWithRetry, hideDashboardRail, navigateToApp, openThreadDrawer, stampOverlayBuild, watchPreferenceReads,
} from './helpers';


test.use({ viewport: { width: 1280, height: 800 } });

const SCALES = [75, 100, 125, 150];

const OPEN_DRAWER_WIDTH = 420;

const EPS = 0.6;

interface RowMetrics {
  rootFontPx: number;
  paneWidth: number;
  toggleRight: number;
  backLeft: number;
  forwardRight: number;
  actionsLeft: number;
}

async function measureRow(page: Page): Promise<RowMetrics | null> {
  return page.evaluate(() => {
    const header = document.querySelector('.desktop-header');
    if (!header) return null;
    const toggle = header.querySelector('.thread-toggle-slot');
    const label = header.querySelector('.pane-header-brand-label');
    const actions = header.querySelector('.pane-header-brand-actions');
    const pane = document.querySelector('.pane-thread');
    if (!toggle || !label || !actions || !pane) return null;
    const chevrons = label.querySelectorAll('.thread-nav-btn');
    if (chevrons.length !== 2) return null;
    const t = toggle.getBoundingClientRect();
    const back = chevrons[0].getBoundingClientRect();
    const forward = chevrons[1].getBoundingClientRect();
    const a = actions.getBoundingClientRect();
    if (t.width === 0 || back.width === 0 || a.width === 0) return null;
    return {
      rootFontPx: parseFloat(getComputedStyle(document.documentElement).fontSize),
      paneWidth: pane.getBoundingClientRect().width,
      toggleRight: t.right,
      backLeft: back.left,
      forwardRight: forward.right,
      actionsLeft: a.left,
    };
  });
}

async function dragDividerTo(page: Page, toX: number): Promise<void> {
  const box = await page.locator('.split-divider').boundingBox();
  if (!box) throw new Error('.split-divider not visible');
  await page.mouse.move(box.x + box.width / 2, 400);
  await page.mouse.down();
  await page.mouse.move(toX, 400, { steps: 5 });
  await page.mouse.up();
}

function expectClear(m: RowMetrics, where: string): void {
  expect(
    m.backLeft,
    `${where}: back chevron starts at ${m.backLeft.toFixed(1)}, `
      + `on a drawer toggle ending at ${m.toggleRight.toFixed(1)} `
      + `(Conversation pane ${m.paneWidth.toFixed(0)}px)`,
  ).toBeGreaterThanOrEqual(m.toggleRight - EPS);
  expect(
    m.forwardRight,
    `${where}: forward chevron ends at ${m.forwardRight.toFixed(1)}, `
      + `on an actions cluster starting at ${m.actionsLeft.toFixed(1)} `
      + `(Conversation pane ${m.paneWidth.toFixed(0)}px)`,
  ).toBeLessThanOrEqual(m.actionsLeft + EPS);
}

async function settleRow(page: Page, wantRootPx: number | null, where: string): Promise<RowMetrics> {
  let last: RowMetrics | null = null;
  await expect
    .poll(async () => {
      const m = await measureRow(page);
      const stable = !!m && !!last && Math.abs(m.backLeft - last.backLeft) < 0.5
        && Math.abs(m.paneWidth - last.paneWidth) < 0.5
        && (wantRootPx === null || Math.abs(m.rootFontPx - wantRootPx) < 0.1);
      last = m;
      return stable;
    }, { timeout: 10_000, message: `the row never settled: ${where}` })
    .toBe(true);
  return last!;
}

async function settleAt(page: Page, scale: number, toX: number): Promise<RowMetrics> {
  await page.evaluate(
    (s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`),
    scale,
  );
  await dragDividerTo(page, toX);
  return settleRow(page, 16 * scale / 100, `ui-scale ${scale}`);
}

test.describe('the Conversation header cluster clears both flanking controls', () => {
  let preferencesSettled: () => Promise<void>;

  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    await hideDashboardRail(page);
    await context.addInitScript((width) => {
      localStorage.setItem('lucidos-split-ratio', '0.4');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.setItem('lucidos-thread-drawer-width', String(width));
    }, OPEN_DRAWER_WIDTH);
    preferencesSettled = watchPreferenceReads(page);
    await navigateToApp(page);
    await preferencesSettled();
  });

  for (const packaged of [true, false]) {
    const build = packaged ? 'packaged macOS' : 'web';

    test(`${build}: at the pane's own floor, drawer shut`, async ({ page }) => {
      if (packaged) await stampOverlayBuild(page);
      for (const scale of SCALES) {
        expectClear(await settleAt(page, scale, 2), `${build}, drawer shut, ui-scale ${scale}`);
      }
    });

    test(`${build}: at the pane's own floor, drawer open`, async ({ page }) => {
      if (packaged) await stampOverlayBuild(page);
      await openThreadDrawer(page);
      await page.waitForFunction((want) => {
        const drawer = document.querySelector('.thread-drawer');
        return !!drawer && Math.abs(drawer.getBoundingClientRect().width - want) < 1;
      }, OPEN_DRAWER_WIDTH, { timeout: 5_000 });

      for (const scale of SCALES) {
        expectClear(await settleAt(page, scale, 2), `${build}, drawer open, ui-scale ${scale}`);
      }
    });

    test(`${build}: a layout persisted UNDER the new floor is migrated on load`, async ({ page }) => {
      await page.evaluate(() => {
        localStorage.setItem('lucidos-split-ratio', '0.234');
      });
      await gotoWithRetry(page, '/');
      await preferencesSettled();
      if (packaged) await stampOverlayBuild(page);

      const m = await settleRow(page, null, 'after the sub-floor migration');
      expect(
        m.paneWidth,
        `the split stayed at the stored ${m.paneWidth.toFixed(0)}px, under its own floor`,
      ).toBeGreaterThan(300);
      expectClear(m, `${build}, migrated from a sub-floor persisted ratio`);
    });

    test(`${build}: and everywhere between the floor and a wide pane`, async ({ page }) => {
      if (packaged) await stampOverlayBuild(page);
      for (const toX of [2, 340, 400, 460, 520, 640, 800]) {
        expectClear(await settleAt(page, 125, toX), `${build}, divider at x=${toX}`);
      }
    });
  }
});
