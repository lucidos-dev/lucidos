/**
 * Regression tests: nothing the mobile threads pane shows at its top may be
 * clipped by the fixed mobile header. Two views live in that pane and each is
 * its own scroll container, so each has to reserve the header height itself:
 * the thread list, and the filter panel that covers it.
 *
 * Test 1 (the list) is about the header's MEASURED height going stale on iOS
 * PWA; test 2 (the filter panel) is about a scroll container missing the
 * spacer entirely. Same visible symptom, different halves of the mechanism.
 * The rest of this block is test 1; test 2 carries its own note at its site.
 *
 * TEST 1. The top of the mobile threads list must not be clipped by the fixed
 * mobile header on iOS PWA.
 *
 * The header is `position: fixed` and sits above the swipe panes. The
 * threads pane's scroll container (`.thread-drawer-list`) uses a `::before`
 * pseudo-element spacer sized to `--mobile-header-height` so the first row
 * starts BELOW the header instead of behind it.
 *
 * Bug seen on iPhone PWA: the first thread row's title text was sliced
 * by the fixed header bar — the top half of the topmost line was hidden
 * behind the header chrome (dot indicator + safe-area-inset).
 *
 * Root cause: `useHideOnScroll`'s `ResizeObserver` watches the header's
 * default observed box (content-box). When `env(safe-area-inset-top)`
 * changes the header's PADDING (iOS PWA cold start, orientation change,
 * notch-vs-no-notch surface changes), the content-box stays the same and
 * the observer NEVER fires — so `--mobile-header-height` keeps the
 * stale value computed without the safe-area inset, and the `::before`
 * spacer comes up tens of pixels short. The first thread row peeks
 * behind the header bottom edge.
 *
 * Chromium's mobile emulator zeroes `env(safe-area-inset-top)`, so the
 * bug never surfaces in the default mobile project. The test simulates
 * iOS PWA by injecting padding-top on the header AFTER mount, which is
 * the same observable transition (content-box unchanged, padding
 * changed) the observer must catch.
 */
import { test, expect } from './fixtures';
import { assertHealthy, clickVisibleElement, ensureMobileView, navigateToApp, sendMessage, uniqueMessage, waitForResponse, waitForVisibleElement } from './helpers';

const SIMULATED_SAFE_AREA_TOP_PX = 59; // iPhone 14 Pro dynamic-island inset

test.describe('Mobile threads pane: top content not clipped under header', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('first row stays below header after safe-area-inset-top appears', async ({ page }) => {
    await navigateToApp(page);

    // Ensure at least one real thread exists so the drawer renders rows.
    const msg = uniqueMessage('threads-top-clip');
    await sendMessage(page, `Say exactly: "top clip ${msg}"`);
    await waitForResponse(page);

    // Switch to the threads pane (pane 0).
    await ensureMobileView(page, 'threads');

    // Wait for the scroll container and at least one row to be rendered.
    await page.waitForFunction(() => {
      const list = document.querySelector('.mobile-swipe-pane .thread-drawer-list');
      const row = document.querySelector('.mobile-swipe-pane .thread-row');
      return !!list && !!row && row.getBoundingClientRect().height > 0;
    }, undefined, { timeout: 10_000 });

    // Simulate iOS PWA safe-area-inset-top by stamping padding-top on the
    // fixed header. This mimics the on-device padding env() resolves to,
    // without needing a real device. iOS PWAs can transition from no-inset
    // to inset (cold-start layout settle, orientation change), and the
    // header's observer must catch that.
    await page.evaluate((px) => {
      const style = document.createElement('style');
      style.id = 'simulate-ios-safe-area';
      style.innerHTML = `.app-header { padding-top: ${px}px !important; }`;
      document.head.appendChild(style);
      // Force a measurement pass — same as iOS PWA would do via its own
      // layout settle. Without this, the test still exercises the bug
      // (ResizeObserver should catch the change), but waiting one tick
      // gives the observer fair opportunity to fire.
      window.dispatchEvent(new Event('resize'));
      const list = document.querySelector('.mobile-swipe-pane .thread-drawer-list') as HTMLElement | null;
      if (list) list.scrollTop = 0;
    }, SIMULATED_SAFE_AREA_TOP_PX);

    // Generous settle: let any rAF-debounced observer/MutationObserver
    // tick before measuring.
    await page.waitForTimeout(300);

    const result = await page.evaluate(() => {
      const header = document.querySelector('.app-header');
      const list = document.querySelector('.mobile-swipe-pane .thread-drawer-list');
      const rows = Array.from(document.querySelectorAll('.mobile-swipe-pane .thread-row'));
      if (!header || !list) return { error: 'missing header or list' };
      const firstRow = rows.find((r) => r.getBoundingClientRect().height > 0);
      if (!firstRow) return { error: 'no visible row' };
      const cssVar = getComputedStyle(document.documentElement).getPropertyValue('--mobile-header-height').trim();
      const remSize = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
      const cssVarPx = cssVar.endsWith('rem')
        ? parseFloat(cssVar) * remSize
        : parseFloat(cssVar) || 0;
      return {
        headerBottom: header.getBoundingClientRect().bottom,
        headerHeight: header.getBoundingClientRect().height,
        rowTop: firstRow.getBoundingClientRect().top,
        listScrollTop: (list as HTMLElement).scrollTop,
        cssVar,
        cssVarPx,
      };
    });

    expect(result).not.toHaveProperty('error');
    const { headerBottom, headerHeight, rowTop, listScrollTop, cssVar, cssVarPx } = result as {
      headerBottom: number; headerHeight: number; rowTop: number;
      listScrollTop: number; cssVar: string; cssVarPx: number;
    };

    // Sanity: list is at top and the safe-area simulation actually grew
    // the header. If the simulation didn't take, the test isn't testing
    // what it claims to.
    expect(listScrollTop).toBe(0);
    expect(headerHeight,
      `header height (${headerHeight}) did not include simulated safe-area (${SIMULATED_SAFE_AREA_TOP_PX}px)`)
      .toBeGreaterThanOrEqual(SIMULATED_SAFE_AREA_TOP_PX);

    // The first row's top must be at or below the header's bottom.
    // 1px subpixel tolerance.
    expect(
      rowTop,
      `first thread row top (${rowTop}) is above header bottom (${headerBottom}) — ` +
      `row is clipped under header. header height=${headerHeight}px, ` +
      `--mobile-header-height=${cssVar} (${cssVarPx}px). ` +
      `Either useHideOnScroll's ResizeObserver missed the padding change ` +
      `(needs box: 'border-box') or the CSS fallback is too short.`,
    ).toBeGreaterThanOrEqual(headerBottom - 1);
  });

  /**
   * The filter panel COVERS the thread list inside the same pane and scrolls
   * on its own (`.thread-filter-panel`, position:absolute inset:0 over
   * `.thread-drawer-list`). So it inherits nothing from the list's header
   * spacer and needs its own, which it shipped without: the panel's first
   * Status rows rendered behind the fixed header.
   *
   * No safe-area simulation here. This half fails with the header at its
   * ordinary height, because the pane starts at the viewport top and a panel
   * with no spacer starts there with it.
   */
  test('filter panel Status rows start below the header', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'threads');

    // Wait for the button to be laid out before clicking it. `ensureMobileView`
    // resolves as soon as the header's data-mobile-view flips, which is what
    // makes the threads row displayable, not what proves it has been measured.
    // Without the wait a loaded run can catch a zero-rect button and fail on
    // "button not visible" instead of on the geometry this test guards.
    await waitForVisibleElement(page, 'button[aria-label="Filter threads"]', 10_000);
    const opened = await clickVisibleElement(page, 'button[aria-label="Filter threads"]');
    expect(opened, 'Filter threads button not visible in the mobile threads header').toBe(true);

    await page.waitForFunction(() => {
      const row = document.querySelector('.mobile-swipe-pane .thread-filter-panel .drawer-view-option');
      return !!row && row.getBoundingClientRect().height > 0;
    }, undefined, { timeout: 10_000 });

    const result = await page.evaluate(() => {
      const header = document.querySelector('.app-header');
      const panel = document.querySelector('.mobile-swipe-pane .thread-filter-panel');
      const firstRow = document.querySelector('.mobile-swipe-pane .thread-filter-panel .drawer-view-option');
      if (!header || !panel || !firstRow) return { error: 'missing header, panel or row' };
      return {
        headerBottom: header.getBoundingClientRect().bottom,
        panelScrollTop: (panel as HTMLElement).scrollTop,
        rowTop: firstRow.getBoundingClientRect().top,
        rowLabel: (firstRow.textContent ?? '').trim(),
      };
    });

    expect(result).not.toHaveProperty('error');
    const { headerBottom, panelScrollTop, rowTop, rowLabel } = result as {
      headerBottom: number; panelScrollTop: number; rowTop: number; rowLabel: string;
    };

    // Sanity: the panel is at its top, so the row's position is its resting
    // one and not the product of a scroll.
    expect(panelScrollTop).toBe(0);

    // 1px subpixel tolerance, as in the list test above.
    expect(
      rowTop,
      `first Status row ("${rowLabel}") top (${rowTop}) is above header bottom ` +
      `(${headerBottom}), so the row is hidden under the fixed header. The panel ` +
      `is a scroll container of its own and must carry the ::before header ` +
      `spacer (mobile.css, the .mobile-swipe-pane …::before group).`,
    ).toBeGreaterThanOrEqual(headerBottom - 1);
  });
});
