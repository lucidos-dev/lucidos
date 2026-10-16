/**
 * The mobile content header's two structural promises, at ui-scale 100 / 125 / 150.
 *
 * 1. **The trailing cluster is bounded at two icon boxes.** Context actions
 *    collapse into the `⋯` overflow menu whatever the width, so the cluster is
 *    one control plus the bell. Three views, three shapes: `⋯` when two or more
 *    actions fold, the action's own icon when exactly one would, and the bell
 *    alone when the view carries none. A second bare context action in the
 *    header is the regression. Not a cosmetic rule: a trailing cluster whose
 *    width tracked the action count moved the centred nav cluster with it. The
 *    chevrons then sat somewhere different on every content view.
 * 2. **The title never paints under either cluster.** It is the one shrinkable
 *    member of a fixed-width centred box, so it ellipsises at the span between
 *    the chevrons rather than crossing an icon.
 *
 * Both replace a measured layout. The header used to fold actions in one at a
 * time and publish `--mobile-content-title-max` / `--mobile-content-title-shift`,
 * sliding the centred box into whichever side had slack, because a
 * variable-width title faced a variable-width trailing cluster and a constant
 * rem reserve cannot clear one at every ui-scale. The scales are still swept
 * here for exactly that reason: the CSS clamp that replaced the measurement has
 * to hold across them too.
 *
 * Three fixtures, one per shape. An app carries three context actions under a
 * ~13-char title, the exact repro from the demo closeup. The Files view carries
 * one. A settings subview carries none, under the longest label in the nav.
 */
import { test, expect, Page } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { assertHealthy, gotoWithRetry, ensureMobileView, navigateToApp, clickVisibleElement, openFilesPanel } from './helpers';

const APP_ID = 'e2e-mobile-title-overlap';
const APP_NAME = 'Half Marathon'; // the demo's ~13-char name, the exact repro
let fixture: { cleanup: () => void };

interface ContentMetrics {
  titleLeft: number;
  titleRight: number;
  titleWidth: number;
  navRight: number;       // leading cluster (the hamburger) inner edge
  actionsLeft: number;    // trailing cluster inner edge
  hasOverflow: boolean;   // the ⋯ overflow trigger is rendered
  bareActions: number;    // context actions rendered as header buttons (bell aside)
  trailingIcons: number;  // everything in the trailing cluster, bell included
  clusterCenter: number;  // the centred box's own centre
  rowCenter: number;      // the axis it is centred on
}

/** Measure the visible mobile content header's centred cluster against its two
 *  icon clusters. Returns null until the title and both clusters are laid out. */
async function measureContent(page: Page): Promise<ContentMetrics | null> {
  return page.evaluate(() => {
    const header = document.querySelector('.mobile-content-header') as HTMLElement | null;
    if (!header || header.getBoundingClientRect().width === 0) return null;
    const title = header.querySelector('.mobile-content-title') as HTMLElement | null;
    // The chevrons live in the centred cluster beside the title, so the
    // hamburger is the whole leading side of this row.
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

/** Wait out the relayout a ui-scale change causes (every control in the row is
 *  rem-sized) before asserting on the boxes. */
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
  return (await measureContent(page))!;
}

/** The two promises that hold for every content view, at every scale. */
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
  // Centred on the ROW middle with no offset of any kind. The measured layout
  // this replaced could slide the box, and pinning that it no longer can is
  // what keeps this pane's chevrons on the same axis as the thread pane's
  // (asserted directly in mobile-threads-title-alignment.spec.ts).
  expect(
    Math.abs(m.clusterCenter - m.rowCenter),
    `ui-scale ${scale}: cluster centre ${m.clusterCenter.toFixed(1)} is off the row centre ${m.rowCenter.toFixed(1)}`,
  ).toBeLessThan(1.5);
}

test.describe('Mobile content header collapses to one icon and never overlaps', () => {
  // iPhone 15 Pro portrait points, the exact device the demo closeup films.
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
    // Restore-on-load opens the app in the content pane (same hook as
    // sdk-iframe-mount): panelOverlay = {type:'app-ui', app}, so the content
    // header shows the app name plus the app-ui actions (refresh, open-in-tab,
    // fullscreen).
    await page.addInitScript((id) => localStorage.setItem('app-window-open', id), APP_ID);
    await gotoWithRetry(page, '/');
    await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible({ timeout: 15_000 });
    await ensureMobileView(page, 'content');
    await expect(page.locator('.mobile-content-header .mobile-content-title')).toBeVisible({ timeout: 10_000 });

    for (const scale of [100, 125, 150]) {
      const m = await setScaleAndSettle(page, scale);
      // The ask, and the reason the trailing cluster is predictable at all: the
      // actions are collapsed even at ui-scale 100, where they would all fit.
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
    // Files carries a single action, Search files. Folding it would cost a tap
    // and save nothing: the ⋯ trigger stands in the same box.
    await navigateToApp(page);
    await openFilesPanel(page);
    await ensureMobileView(page, 'content');
    await expect(page.locator('.mobile-content-header .mobile-content-title'))
      .toHaveText('Files', { timeout: 15_000 });

    for (const scale of [100, 125, 150]) {
      const m = await setScaleAndSettle(page, scale);
      expect(m.hasOverflow, `ui-scale ${scale}: a lone action needs no overflow trigger`).toBe(false);
      expect(m.bareActions, `ui-scale ${scale}: the one action should ride the row`).toBe(1);
      // The bound the centred cluster's edge reserve is sized against: the same
      // two boxes the overflow shape costs, so the chevrons do not move.
      expect(m.trailingIcons, `ui-scale ${scale}: the trailing cluster should be the action + bell`).toBe(2);
      expectClearAndCentred(m, scale);
    }

    // ...and it is the search action, reachable without opening anything.
    await expect(page.locator('.mobile-content-header .file-search-btn.icon-btn')).toBeVisible();
  });

  test('a view with NO context actions shows the bell alone', async ({ page }) => {
    await navigateToApp(page);

    // Tapped through the way a user reaches it (menu drawer, Settings, the
    // subview row), NOT via POST /api/v1/ui/navigate: that route is delivered
    // over SSE, and this assertion is about layout, so depending on the stream
    // would only buy a second way to fail. Appearance & Behavior carries the
    // longest label in the nav, which is why it is also the one that authored a
    // header shorthand.
    await clickVisibleElement(page, '.hamburger-panel');
    await expect
      .poll(() => clickVisibleElement(page, '.drawer-item', 'Settings'), { timeout: 10_000 })
      .toBe(true);
    await ensureMobileView(page, 'content');
    await expect
      .poll(() => clickVisibleElement(page, '.settings-nav-row', 'Appearance & Behavior'), { timeout: 10_000 })
      .toBe(true);

    // The bar shows the shorthand; the row the user just tapped still carries
    // the full name, and so does the title's tap tooltip. Asserted here rather
    // than in a unit test because the point of a shorthand is that it survives
    // real font metrics at a real viewport: the full name does not.
    const title = page.locator('.mobile-content-header .mobile-content-title');
    await expect(title).toHaveText('Appearance', { timeout: 15_000 });
    await expect(title).toHaveAttribute('data-tooltip', 'Appearance & Behavior');

    for (const scale of [100, 125, 150]) {
      const m = await setScaleAndSettle(page, scale);
      // Nothing to collapse, so no trigger for an empty menu.
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
