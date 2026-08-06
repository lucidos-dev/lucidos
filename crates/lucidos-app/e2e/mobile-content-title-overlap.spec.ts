/**
 * Mobile content-header title must NEVER paint under the trailing action icons.
 *
 * The content title is ABSOLUTELY centered on the row middle (see
 * `.mobile-content-title`), so a long title clears the trailing cluster
 * (refresh / open-in-tab / fullscreen / notifications bell) only if a symmetric
 * reserve fits it. A constant rem reserve can't be right at every ui-scale (the
 * cluster is rem-sized while the 393pt viewport is fixed), so
 * `useHeaderActionCollapse`'s mobile branch measures the real geometry, collapses
 * the nearest-title actions into the ⋮ overflow menu to widen the reserve, then
 * slides the box off centre into whichever side still has slack, and only
 * ellipsizes once it has taken every pixel between the two clusters.
 *
 * Two bugs are pinned here across ui-scale 100 / 125 / 150. The demo one: an app
 * named "Half Marathon" at ui-scale 125 on a 393pt viewport, whose title used to
 * reach under the Refresh glyph. And the one the slide exists for: a settings
 * subview, whose bell-only trailing cluster left the reserve sized off the
 * LEADING cluster, truncating the title next to an empty half-row. The pure
 * collapse math is unit-tested in `src/hooks/useHeaderActionCollapse.test.ts`;
 * this exercises the DOM wiring (measurement → `--mobile-content-title-max` +
 * `--mobile-content-title-shift` → the painted box, plus the collapse).
 */
import { test, expect, Page } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { assertHealthy, gotoWithRetry, ensureMobileView, navigateToApp, clickVisibleElement } from './helpers';

const APP_ID = 'e2e-mobile-title-overlap';
const APP_NAME = 'Half Marathon'; // the demo's ~13-char name — the exact repro
let fixture: { cleanup: () => void };

interface ContentMetrics {
  titleLeft: number;
  titleRight: number;
  titleWidth: number;
  navRight: number;      // leading cluster (hamburger + nav slot) inner edge
  actionsLeft: number;   // trailing cluster (refresh/…/bell) inner edge
  hasOverflow: boolean;  // the ⋮ overflow trigger is rendered
  truncated: boolean;    // the title is ellipsized (clamped below its content)
  actionCount: number;   // context actions in the trailing cluster (bell aside)
  titleCenter: number;   // painted box centre
  rowCenter: number;     // the axis the box is centred on before the shift
  shift: number;         // --mobile-content-title-shift, the published offset
}

/** Measure the visible mobile content header's centered title against its two
 *  icon clusters. Returns null until the title + both clusters are laid out. */
async function measureContent(page: Page): Promise<ContentMetrics | null> {
  return page.evaluate(() => {
    const header = document.querySelector('.mobile-content-header') as HTMLElement | null;
    if (!header || header.getBoundingClientRect().width === 0) return null;
    const title = header.querySelector('.mobile-content-title') as HTMLElement | null;
    const nav = header.querySelector('.mobile-nav-slot') as HTMLElement | null;
    const actions = header.querySelector('.content-header-actions') as HTMLElement | null;
    const row = header.querySelector('.mobile-header-row') as HTMLElement | null;
    if (!title || !nav || !actions || !row) return null;
    const t = title.getBoundingClientRect();
    const a = actions.getBoundingClientRect();
    const r = row.getBoundingClientRect();
    if (t.width === 0 || a.width === 0) return null;
    return {
      titleLeft: t.left,
      titleRight: t.right,
      titleWidth: t.width,
      titleCenter: t.left + t.width / 2,
      rowCenter: r.left + r.width / 2,
      shift: parseFloat(getComputedStyle(row).getPropertyValue('--mobile-content-title-shift')) || 0,
      navRight: nav.getBoundingClientRect().right,
      actionsLeft: a.left,
      hasOverflow: !!header.querySelector('.content-header-more'),
      truncated: title.clientWidth + 0.5 < title.scrollWidth,
      actionCount: actions.querySelectorAll('.icon-btn.header-icon').length
        - actions.querySelectorAll('.notifications-bell').length,
    };
  });
}

/** The box sits on the row middle, offset by exactly the published shift, and
 *  by nothing else. Both halves need pinning and neither is implied by the
 *  edge assertions: a box that stopped centring (the CSS centres it with
 *  `left/right: 0` + `margin-inline: auto`, not the shared `left: 50%`) or a
 *  shift applied twice or with the wrong sign can still land between the two
 *  clusters and satisfy them. */
function expectCenteredOnRowPlusShift(m: ContentMetrics, scale: number): void {
  expect(
    Math.abs(m.titleCenter - (m.rowCenter + m.shift)),
    `ui-scale ${scale}: title centre ${m.titleCenter.toFixed(1)} is not the row centre `
      + `${m.rowCenter.toFixed(1)} offset by the published shift ${m.shift.toFixed(1)}`,
  ).toBeLessThan(1);
}

test.describe('Mobile content title never overlaps the header actions', () => {
  // iPhone 15 Pro portrait points — the exact device the demo closeup films.
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

  test('long title truncates/collapses clear of both clusters at ui-scale 100/125/150', async ({ page }) => {
    // Restore-on-load opens the app in the content pane (same hook as
    // sdk-iframe-mount): panelOverlay = {type:'app-ui', app} → the content header
    // shows the app name as its title plus the app-ui trailing actions.
    await page.addInitScript((id) => localStorage.setItem('app-window-open', id), APP_ID);
    await gotoWithRetry(page, '/');
    await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible({ timeout: 15_000 });
    await ensureMobileView(page, 'content');
    await expect(page.locator('.mobile-content-header .mobile-content-title')).toBeVisible({ timeout: 10_000 });

    const overflowByScale: Record<number, boolean> = {};
    for (const scale of [100, 125, 150]) {
      // A ui-scale change resizes the rem-sized row (its icons/min-height), which
      // fires the mobile branch's ResizeObserver → re-measure. Poll until the
      // collapse + reserve settle so the assertions aren't racing the observer.
      await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`), scale);
      await expect
        .poll(async () => {
          const m = await measureContent(page);
          if (!m) return false;
          return m.titleWidth > 0
            && m.titleRight <= m.actionsLeft + 0.6
            && m.titleLeft >= m.navRight - 0.6;
        }, { timeout: 6_000, message: `content title never settled clear of the clusters at ui-scale ${scale}` })
        .toBe(true);

      const m = (await measureContent(page))!;
      expect(
        m.titleRight,
        `ui-scale ${scale}: title right ${m.titleRight.toFixed(1)} paints under actions left ${m.actionsLeft.toFixed(1)}`,
      ).toBeLessThanOrEqual(m.actionsLeft + 0.6);
      expect(
        m.titleLeft,
        `ui-scale ${scale}: title left ${m.titleLeft.toFixed(1)} paints under nav right ${m.navRight.toFixed(1)}`,
      ).toBeGreaterThanOrEqual(m.navRight - 0.6);
      expect(m.titleWidth, `ui-scale ${scale}: title has zero width (hidden)`).toBeGreaterThan(0);
      expectCenteredOnRowPlusShift(m, scale);
      overflowByScale[scale] = m.hasOverflow;
    }

    // The mechanism, not just a CSS clamp: at the tightest scale the trailing
    // actions must have folded into the ⋮ overflow to make room for the title.
    expect(
      overflowByScale[150],
      `overflow ⋮ should engage at ui-scale 150 (collapse states seen: ${JSON.stringify(overflowByScale)})`,
    ).toBe(true);
  });

  // The reported bug, and the reason the box may sit off centre at all. A
  // settings subview carries NO context actions, so the trailing cluster is the
  // bell alone while the leading one is hamburger + back/forward. The symmetric
  // reserve is sized off whichever cluster is NEARER the centre, so it was sized
  // off the leading side and truncated the title with the whole trailing half of
  // the row empty ("Appearance & Beha…" next to blank space).
  test('a bell-only trailing cluster spends its slack on the title instead of truncating', async ({ page }) => {
    await navigateToApp(page);

    // Tapped through the way the report did (menu drawer → Settings → the
    // subview row), NOT via POST /api/v1/ui/navigate: that route is delivered
    // over SSE, and this assertion is about layout, so depending on the stream
    // would only buy a second way to fail. Appearance & Behavior is the exact
    // subview the report screenshots, and its label is the longest in the nav.
    await clickVisibleElement(page, '.hamburger-panel');
    await expect
      .poll(() => clickVisibleElement(page, '.drawer-item', 'Settings'), { timeout: 10_000 })
      .toBe(true);
    await ensureMobileView(page, 'content');
    await expect
      .poll(() => clickVisibleElement(page, '.settings-nav-row', 'Appearance & Behavior'), { timeout: 10_000 })
      .toBe(true);

    await expect(page.locator('.mobile-content-header .mobile-content-title'))
      .toHaveText('Appearance & Behavior', { timeout: 15_000 });

    for (const scale of [100, 125, 150]) {
      // A ui-scale change re-lays out the rem-sized row (icons, min-height, the
      // title's own font), so the published box is momentarily the PREVIOUS
      // scale's until the hook's ResizeObserver re-measures. Poll on every edge
      // this test asserts, not just one: polling the trailing edge alone let a
      // box still carrying the old shift through, and it was overhanging the
      // (now wider) leading cluster by 25px on WebKit.
      await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`), scale);
      await expect
        .poll(async () => {
          const m = await measureContent(page);
          return !!m && m.titleWidth > 0
            && m.titleRight <= m.actionsLeft + 0.6
            && m.titleLeft >= m.navRight - 0.6;
        }, { timeout: 6_000, message: `content title never settled clear of both clusters at ui-scale ${scale}` })
        .toBe(true);

      const m = (await measureContent(page))!;
      // The shape under test: nothing to fold into ⋮ here, so widening the box
      // is the ONLY move available.
      expect(m.actionCount, `ui-scale ${scale}: settings should carry no context actions`).toBe(0);
      expect(m.hasOverflow, `ui-scale ${scale}: nothing to collapse, so no ⋮`).toBe(false);
      // Never under the icons, either side.
      expect(
        m.titleRight,
        `ui-scale ${scale}: title right ${m.titleRight.toFixed(1)} paints under actions left ${m.actionsLeft.toFixed(1)}`,
      ).toBeLessThanOrEqual(m.actionsLeft + 0.6);
      expect(
        m.titleLeft,
        `ui-scale ${scale}: title left ${m.titleLeft.toFixed(1)} paints under nav right ${m.navRight.toFixed(1)}`,
      ).toBeGreaterThanOrEqual(m.navRight - 0.6);
      expectCenteredOnRowPlusShift(m, scale);
      // The regression itself: an ellipsis is only allowed once the box has
      // taken every pixel between the two clusters. Truncating with a gap on
      // either side is the bug.
      if (m.truncated) {
        expect(
          m.titleLeft,
          `ui-scale ${scale}: title truncated with ${(m.titleLeft - m.navRight).toFixed(1)}px unused on the leading side`,
        ).toBeLessThanOrEqual(m.navRight + 1);
        expect(
          m.titleRight,
          `ui-scale ${scale}: title truncated with ${(m.actionsLeft - m.titleRight).toFixed(1)}px unused on the trailing side`,
        ).toBeGreaterThanOrEqual(m.actionsLeft - 1);
      }
    }
  });

  test('the Lucidos brand (thread) header is unaffected — centered and clear of its leading icons', async ({ page }) => {
    await gotoWithRetry(page, '/');
    await ensureMobileView(page, 'thread');
    await page.evaluate(() => document.documentElement.style.setProperty('--user-ui-scale', '125%'));

    const brand = await page.evaluate(() => {
      const header = document.querySelector('.mobile-thread-header') as HTMLElement | null;
      const row = header?.querySelector('.mobile-header-row') as HTMLElement | null;
      const brandEl = header?.querySelector('.pane-header-brand') as HTMLElement | null;
      const nav = header?.querySelector('.mobile-nav-slot') as HTMLElement | null;
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
    expect(brand, 'brand / nav slot not found').not.toBeNull();
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
