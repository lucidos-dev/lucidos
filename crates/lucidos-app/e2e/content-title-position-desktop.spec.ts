import { test, expect, type Page } from './fixtures';
import { createIframeAppFixture, ensureHomeThread } from './db-helpers';
import { assertHealthy, gotoWithRetry, clickVisibleElement, hideDashboardRail, watchPreferenceReads, openSettingsView } from './helpers';

const APP_ID = 'e2e-desktop-title-position';
const APP_NAME = 'Trip countdown';
const APP_ID_LONG = 'e2e-desktop-title-position-long';
const APP_NAME_LONG = 'Half Marathon Training Plan and Weekly Readiness Review';
let fixture: { cleanup: () => void };
let fixtureLong: { cleanup: () => void };
let widgetFixture: { cleanup: () => void };

test.use({ viewport: { width: 1280, height: 800 } });

interface HeaderMetrics {
  backX: number;
  forwardX: number;
  boxCentre: number;
  boxRight: number;
  boxWidth: number;
  rowCentre: number;
  rowWidth: number;
  navRight: number;
  hasRefresh: boolean;
  actionsLeft: number;
  minSpan: number;
  sideReserve: number;
  span: number;
  spanMax: number;
  unfoldedReserve: number;
  titleText: string;
  hasOverflow: boolean;
  trailingIcons: number;
}

async function measure(page: Page): Promise<HeaderMetrics | null> {
  return page.evaluate(() => {
    const row = document.querySelector('.desktop-header .content-header-elements') as HTMLElement | null;
    if (!row || row.getBoundingClientRect().width === 0) return null;
    const box = row.querySelector('.pane-header-content-title') as HTMLElement | null;
    const back = row.querySelector('.content-back-btn') as HTMLElement | null;
    const forward = row.querySelector('.content-forward-btn') as HTMLElement | null;
    const nav = row.querySelector('.hamburger-panel') as HTMLElement | null;
    const actions = row.querySelector('.content-header-actions') as HTMLElement | null;
    const refresh = row.querySelector('.content-refresh-btn') as HTMLElement | null;
    if (!box || !back || !forward || !nav || !actions) return null;
    const b = box.getBoundingClientRect();
    const r = row.getBoundingClientRect();
    const a = actions.getBoundingClientRect();
    if (b.width === 0 || a.width === 0) return null;
    const resolve = (token: string) => {
      const probe = document.createElement('div');
      probe.style.cssText = `position:absolute;visibility:hidden;width:var(${token})`;
      row.appendChild(probe);
      const px = probe.getBoundingClientRect().width;
      probe.remove();
      return px;
    };
    const compact = actions.hasAttribute('data-compact');
    const more = actions.querySelector<HTMLElement>('.content-header-more');
    const visibleRefresh = refresh && refresh.getBoundingClientRect().width > 0 ? refresh : null;
    const leading = compact ? (more ?? row.querySelector('[data-compact-action]') ?? visibleRefresh ?? nav) : (visibleRefresh ?? nav);
    const minSpan = resolve('--desktop-nav-min-span');
    const sideReserve = resolve('--content-side-reserve');
    return {
      backX: back.getBoundingClientRect().left,
      forwardX: forward.getBoundingClientRect().right,
      boxCentre: b.left + b.width / 2,
      boxRight: b.right,
      boxWidth: b.width,
      rowCentre: r.left + r.width / 2,
      rowWidth: r.width,
      navRight: leading.getBoundingClientRect().right,
      hasRefresh: visibleRefresh !== null,
      actionsLeft: a.left,
      minSpan,
      sideReserve,
      span: resolve('--desktop-nav-span'),
      spanMax: resolve('--desktop-content-title-span-max'),
      unfoldedReserve: resolve('--content-unfolded-reserve'),
      titleText: (box.querySelector('.pane-header-title-text')?.textContent ?? '').trim(),
      hasOverflow: actions.querySelectorAll('.content-header-more').length > 0,
      trailingIcons: actions.querySelectorAll('.icon-btn.header-icon').length - (compact && more ? 1 : 0),
    };
  });
}

async function settled(page: Page): Promise<HeaderMetrics> {
  let last: HeaderMetrics | null = null;
  await expect.poll(async () => {
    const now = await measure(page);
    const stable = now !== null && last !== null
      && Math.abs(now.actionsLeft - last.actionsLeft) < 0.5
      && Math.abs(now.boxCentre - last.boxCentre) < 0.5;
    last = now;
    return stable;
  }, { timeout: 10_000 }).toBe(true);
  return last!;
}

function expectCentredAndClear(m: HeaderMetrics, view: string): void {
  expect(
    Math.abs(m.boxCentre - m.rowCentre),
    `${view}: box centre ${m.boxCentre.toFixed(1)} is off the row centre ${m.rowCentre.toFixed(1)}`,
  ).toBeLessThan(1.5);
  expect(
    m.boxRight,
    `${view}: the title box reaches the actions cluster at ${m.actionsLeft.toFixed(1)}`,
  ).toBeLessThanOrEqual(m.actionsLeft + 0.6);
  expect(
    m.backX,
    `${view}: the back chevron reaches the leading cluster at ${m.navRight.toFixed(1)}`,
  ).toBeGreaterThanOrEqual(m.navRight - 0.6);
}

test.describe('the desktop content title holds its position across views', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: APP_NAME, description: 'e2e fixture' },
      html: `<!DOCTYPE html><html><head><meta charset="UTF-8"><title>${APP_NAME}</title></head><body><div id="ready">ready</div></body></html>`,
      js: '',
    });
    widgetFixture = createIframeAppFixture(`${APP_ID}-widget`, {
      manifest: { name: APP_NAME, kind: 'widget', reusable: true },
      html: '<!DOCTYPE html><html><body>Trip countdown widget</body></html>',
      js: '',
    });
    fixtureLong = createIframeAppFixture(APP_ID_LONG, {
      manifest: { id: APP_ID_LONG, name: APP_NAME_LONG, description: 'e2e fixture' },
      html: `<!DOCTYPE html><html><head><meta charset="UTF-8"><title>${APP_NAME_LONG}</title></head><body><div id="ready">ready</div></body></html>`,
      js: '',
    });
  });

  test.afterAll(() => {
    fixture.cleanup();
    fixtureLong.cleanup();
    widgetFixture.cleanup();
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    ensureHomeThread();
    await hideDashboardRail(page);
  });

  test('a real widget preview at 175% keeps its name whole without a widget suffix', async ({ page }) => {
    await page.setViewportSize({ width: 1920, height: 1080 });
    await page.addInitScript((id) => {
      localStorage.setItem('app-window-open', id);
      localStorage.setItem('lucidos-thread-drawer-open', 'true');
      localStorage.setItem('lucidos-split-ratio', '0.6');
    }, `${APP_ID}-widget`);
    const preferencesReady = watchPreferenceReads(page);
    await gotoWithRetry(page, '/');
    await preferencesReady();
    await page.evaluate(() => document.documentElement.style.setProperty('--user-ui-scale', '175%'));
    await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible();
    await settled(page);
    const title = page.locator('.desktop-header .pane-header-title-text');
    await expect(title).toBeVisible();
    expect.soft(await title.evaluate(el => el.scrollWidth <= el.clientWidth), 'widget name fits').toBe(true);
    await expect.soft(title).not.toContainText('(widget)');
    await expect(title).toHaveText(APP_NAME);
  });

  test('compact overflow holds its leading slot during position-only layout changes', async ({ page }) => {
    await page.setViewportSize({ width: 1920, height: 1080 });
    await page.addInitScript((id) => {
      localStorage.setItem('app-window-open', id);
      localStorage.setItem('lucidos-thread-drawer-open', 'true');
      localStorage.setItem('lucidos-split-ratio', '0.6');
    }, APP_ID);
    const preferencesReady = watchPreferenceReads(page);
    await gotoWithRetry(page, '/');
    await preferencesReady();
    await page.evaluate(() => document.documentElement.style.setProperty('--user-ui-scale', '175%'));
    await settled(page);
    await expect(page.locator('.desktop-header .content-header-actions')).toHaveAttribute('data-compact');
    const more = page.locator('.desktop-header .content-header-more');
    await expect(more).toBeVisible();
    const shifts = await more.evaluate((el) => {
      const row = el.closest<HTMLElement>('.content-header-elements')!;
      const hamburger = row.querySelector('.hamburger-panel')!;
      const style = getComputedStyle(row);
      const padding = parseFloat(style.paddingRight);
      const gap = parseFloat(style.gap);
      return [padding + 40, padding].map((right) => {
        row.style.paddingRight = `${right}px`;
        return el.getBoundingClientRect().left - hamburger.getBoundingClientRect().right - gap;
      });
    });
    for (const shift of shifts) expect(Math.abs(shift), 'overflow stays beside the hamburger').toBeLessThan(1);
    await more.click();
    await expect(page.getByRole('menuitem', { name: 'Refresh', exact: true })).toBeVisible();
  });

  test('reported 1920 by 1080 at 175% keeps real destinations whole', async ({ page }) => {
    await page.setViewportSize({ width: 1920, height: 1080 });
    await page.addInitScript((id) => {
      localStorage.setItem('app-window-open', id);
      localStorage.setItem('lucidos-thread-drawer-open', 'true');
      localStorage.setItem('lucidos-split-ratio', '0.6');
    }, APP_ID);
    const preferencesReady = watchPreferenceReads(page);
    const scale = async () => {
      await preferencesReady();
      await page.evaluate(() => document.documentElement.style.setProperty('--user-ui-scale', '175%'));
    };
    await gotoWithRetry(page, '/');
    await scale();
    await expect(page.locator('.dashboard-rail-toggle')).toBeVisible();
    for (const shown of [false, true]) {
      if (shown) await page.locator('.dashboard-rail-toggle').click();
      for (const destination of ['Apps', 'Memory', APP_NAME]) {
        if (destination === 'Memory') {
          await openSettingsView(page, 'memory');
        } else {
          await clickVisibleElement(page, '.hamburger-panel');
          await expect.poll(() => clickVisibleElement(page, '.drawer-item', 'Apps')).toBe(true);
          if (destination === APP_NAME) {
            await page.locator('.apps-view .app-row', { hasText: APP_NAME })
              .filter({ hasNotText: APP_NAME_LONG }).locator('.app-row-description').click();
            await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible();
          }
        }
        await expect.poll(() => page.evaluate(() =>
          document.documentElement.hasAttribute('data-dashboard-rail'))).toBe(shown);
        await expect(page.locator('.pane-content')).toBeVisible();
        const header = await settled(page);
        if (!shown) {
          expect(header.backX, 'back clears leading overflow').toBeGreaterThanOrEqual(header.navRight - 0.6);
          expect(header.boxRight, 'forward clears trailing controls').toBeLessThanOrEqual(header.actionsLeft + 0.6);
          const more = page.locator('.desktop-header .content-header-more');
          if (destination === 'Memory') {
            await expect(more).toHaveCount(0);
            await expect(page.locator('.desktop-header').getByRole('button', { name: 'Refresh', exact: true })).toBeVisible();
          } else {
            await more.click();
            await expect(page.getByRole('menuitem', { name: 'Refresh', exact: true })).toBeVisible();
            await page.keyboard.press('Escape');
          }
        }
        const title = page.locator('.desktop-header .pane-header-title-text');
        await expect(title).toBeVisible();
        await expect(title).toHaveText(destination);
        const metrics = await title.evaluate((el) => {
          const r = el.getBoundingClientRect();
          const pane = document.querySelector('.pane-content')!.getBoundingClientRect();
          const range = document.createRange();
          range.selectNodeContents(el);
          return { available: el.clientWidth, natural: el.scrollWidth,
            textWidth: range.getBoundingClientRect().width,
            offset: r.x + r.width / 2 - pane.x - pane.width / 2 };
        });
        console.log(JSON.stringify({ destination, shown, ...metrics, header }));
        expect.soft(metrics.natural, `${destination}, rail ${shown}`).toBeLessThanOrEqual(metrics.available + 1);
        expect.soft(Math.abs(metrics.offset), `${destination} pane centering`).toBeLessThan(1.5);
      }
    }
  });

  test('the chevrons land on the same x for a short and a long title', async ({ page }) => {
    const titled: Partial<Record<'short' | 'long', HeaderMetrics>> = {};
    await gotoWithRetry(page, '/');
    for (const [label, id] of [['short', APP_ID], ['long', APP_ID_LONG]] as const) {
      await page.evaluate((appId) => {
        localStorage.removeItem('lucidos-nav-history');
        localStorage.setItem('app-window-open', appId);
      }, id);
      await gotoWithRetry(page, '/');
      await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible({ timeout: 15_000 });
      titled[label] = await settled(page);
    }

    expect(
      titled.long!.titleText.length,
      `the long fixture rendered "${titled.long!.titleText}", no longer than the short one`,
    ).toBeGreaterThan(titled.short!.titleText.length + 20);
    expect(titled.long!.trailingIcons, 'both views carry the same cluster').toBe(titled.short!.trailingIcons);
    expect(titled.long!.actionsLeft, 'the clusters differ, so this is not a title-only comparison')
      .toBeCloseTo(titled.short!.actionsLeft, 0);

    for (const edge of ['backX', 'forwardX', 'boxWidth'] as const) {
      expect(
        titled.long![edge],
        `${edge} moved to ${titled.long![edge].toFixed(1)} from ${titled.short![edge].toFixed(1)} `
          + `on a longer title alone`,
      ).toBeCloseTo(titled.short![edge], 0);
    }
    expectCentredAndClear(titled.long!, 'long title');
  });

  test('the chevrons land on the same x whatever the trailing cluster holds', async ({ page }) => {
    await page.addInitScript((id) => localStorage.setItem('app-window-open', id), APP_ID);
    await gotoWithRetry(page, '/');
    await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible({ timeout: 15_000 });
    const widest = await settled(page);
    expect(widest.hasOverflow, 'the box is at its span cap, so three actions fit and nothing folds').toBe(false);
    expect(widest.trailingIcons, 'the app view: open in tab + find + fullscreen + the bell').toBe(4);
    expect(widest.hasRefresh, 'the app view leads its row with Refresh').toBe(true);
    expectCentredAndClear(widest, 'app UI');

    await clickVisibleElement(page, '.hamburger-panel');
    await expect.poll(() => clickVisibleElement(page, '.drawer-item', 'Apps'), { timeout: 10_000 }).toBe(true);
    const oneAction = await settled(page);
    expect(oneAction.hasOverflow, 'one action does not fold: ⋯ would replace it 1:1').toBe(false);
    expectCentredAndClear(oneAction, 'apps');

    await clickVisibleElement(page, '.hamburger-panel');
    await expect.poll(() => clickVisibleElement(page, '.drawer-item', 'Settings'), { timeout: 10_000 }).toBe(true);
    const noActions = await settled(page);
    expect(noActions.hasOverflow, 'nothing to collapse, so no ⋯').toBe(false);
    expect(noActions.trailingIcons, 'settings: the bell alone').toBe(1);
    expectCentredAndClear(noActions, 'settings');

    expect(
      noActions.actionsLeft,
      'the three views produced the same trailing cluster, so this proves nothing',
    ).toBeGreaterThan(widest.actionsLeft + 1);
    for (const [view, m] of [['apps', oneAction], ['settings', noActions]] as const) {
      expect(
        m.backX,
        `${view}: the back chevron moved to ${m.backX.toFixed(1)} from ${widest.backX.toFixed(1)}`,
      ).toBeCloseTo(widest.backX, 0);
      expect(
        m.forwardX,
        `${view}: the forward chevron moved to ${m.forwardX.toFixed(1)} from ${widest.forwardX.toFixed(1)}`,
      ).toBeCloseTo(widest.forwardX, 0);
    }
  });

  test('the box still clears the cluster with the Canvas pane near its floor', async ({ page }) => {
    await page.addInitScript((id) => {
      localStorage.setItem('lucidos-split-ratio', '0.7');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
    await gotoWithRetry(page, '/');
    await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible({ timeout: 15_000 });

    const narrow = await settled(page);
    const paneWidth = await page.evaluate(
      () => document.querySelector('.pane-content')!.getBoundingClientRect().width,
    );
    expect(paneWidth, 'the seeded ratio did not narrow the Canvas pane').toBeLessThan(400);
    expect(paneWidth, 'the seeded ratio went under the Canvas pane floor').toBeGreaterThan(360);
    const compactWidth = Math.min(narrow.span, Math.max(0, narrow.rowWidth - 2 * narrow.sideReserve));
    expect(narrow.boxWidth, 'compact span clears symmetric control reserves').toBeCloseTo(compactWidth, 0);
    expect(narrow.trailingIcons, 'only the bell remains trailing').toBe(1);
    expect(narrow.hasOverflow, 'optional actions remain accessible').toBe(true);
    expect(narrow.hasRefresh, 'Refresh folds into the leading menu').toBe(false);
    expectCentredAndClear(narrow, 'canvas pane near its floor');
  });

  test('a wide Canvas pane grows the title box, up to its cap', async ({ page }) => {
    await page.setViewportSize({ width: 1920, height: 1000 });
    const atRatio = async (ratio: string): Promise<HeaderMetrics> => {
      await page.evaluate(({ r, id }) => {
        localStorage.removeItem('lucidos-nav-history');
        localStorage.setItem('lucidos-split-ratio', r);
        localStorage.setItem('lucidos-thread-drawer-open', 'false');
        localStorage.setItem('app-window-open', id);
      }, { r: ratio, id: APP_ID_LONG });
      await gotoWithRetry(page, '/');
      await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible({ timeout: 15_000 });
      return settled(page);
    };
    await gotoWithRetry(page, '/');

    const growing = await atRatio('0.45');
    expect(growing.boxWidth, 'the box did not grow past the shared span').toBeGreaterThan(growing.span + 1);
    expect(
      growing.boxWidth,
      `the box is ${growing.boxWidth.toFixed(1)} wide, not the room past the unfolded reserve`,
    ).toBeCloseTo(growing.rowWidth - 2 * growing.unfoldedReserve, 0);
    expect(growing.hasOverflow, 'growing folded the app view\'s actions').toBe(false);
    expectCentredAndClear(growing, 'growing canvas pane');

    const capped = await atRatio('0.3');
    expect(capped.rowWidth - 2 * capped.unfoldedReserve, 'the seeded ratio did not reach the cap')
      .toBeGreaterThan(capped.spanMax);
    expect(capped.boxWidth, 'the box grew past its cap').toBeCloseTo(capped.spanMax, 0);
    expect(capped.hasOverflow).toBe(false);
    expectCentredAndClear(capped, 'capped canvas pane');
  });
});
