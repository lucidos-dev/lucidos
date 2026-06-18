/**
 * Regression test: the top of the mobile threads list must not be clipped
 * by the fixed mobile header on iOS PWA.
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
import { assertHealthy, ensureMobileView, navigateToApp, sendMessage, uniqueMessage, waitForResponse } from './helpers';

const SIMULATED_SAFE_AREA_TOP_PX = 59; // iPhone 14 Pro dynamic-island inset

test.describe('Mobile threads list — top row not clipped under header', () => {
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
});
