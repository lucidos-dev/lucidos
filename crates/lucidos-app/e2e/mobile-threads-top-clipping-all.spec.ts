import { test, expect } from './fixtures';
import { assertHealthy, clickVisibleElement, ensureMobileView, navigateToApp, sendMessage, uniqueMessage, waitForResponse, waitForVisibleElement } from './helpers';

const SIMULATED_SAFE_AREA_TOP_PX = 59;

test.describe('Mobile threads pane: top content not clipped under header', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('the first item stays below header after safe-area-inset-top appears', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('threads-top-clip');
    await sendMessage(page, `Say exactly: "top clip ${msg}"`);
    await waitForResponse(page);

    await ensureMobileView(page, 'threads');

    await page.waitForFunction(() => {
      const list = document.querySelector('.mobile-swipe-pane .thread-drawer-list');
      const row = document.querySelector('.mobile-swipe-pane .thread-row');
      return !!list && !!row && row.getBoundingClientRect().height > 0;
    }, undefined, { timeout: 10_000 });

    await page.evaluate((px) => {
      const style = document.createElement('style');
      style.id = 'simulate-ios-safe-area';
      style.innerHTML = `.app-header { padding-top: ${px}px !important; }`;
      document.head.appendChild(style);
      window.dispatchEvent(new Event('resize'));
      const list = document.querySelector('.mobile-swipe-pane .thread-drawer-list') as HTMLElement | null;
      if (list) list.scrollTop = 0;
    }, SIMULATED_SAFE_AREA_TOP_PX);

    await page.waitForTimeout(300);

    const result = await page.evaluate(() => {
      const header = document.querySelector('.app-header');
      const list = document.querySelector('.mobile-swipe-pane .thread-drawer-list');
      const items = Array.from(document.querySelectorAll('.mobile-swipe-pane .thread-drawer-tree :is(.list-section-title, .thread-row)'));
      if (!header || !list) return { error: 'missing header or list' };
      const firstItem = items.find((r) => r.getBoundingClientRect().height > 0);
      if (!firstItem) return { error: 'no visible item' };
      const cssVar = getComputedStyle(document.documentElement).getPropertyValue('--mobile-header-height').trim();
      const remSize = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
      const cssVarPx = cssVar.endsWith('rem')
        ? parseFloat(cssVar) * remSize
        : parseFloat(cssVar) || 0;
      return {
        headerBottom: header.getBoundingClientRect().bottom,
        headerHeight: header.getBoundingClientRect().height,
        itemTop: firstItem.getBoundingClientRect().top,
        listScrollTop: (list as HTMLElement).scrollTop,
        cssVar,
        cssVarPx,
      };
    });

    expect(result).not.toHaveProperty('error');
    const { headerBottom, headerHeight, itemTop, listScrollTop, cssVar, cssVarPx } = result as {
      headerBottom: number; headerHeight: number; itemTop: number;
      listScrollTop: number; cssVar: string; cssVarPx: number;
    };

    expect(listScrollTop).toBe(0);
    expect(headerHeight,
      `header height (${headerHeight}) did not include simulated safe-area (${SIMULATED_SAFE_AREA_TOP_PX}px)`)
      .toBeGreaterThanOrEqual(SIMULATED_SAFE_AREA_TOP_PX);

    expect(
      itemTop,
      `first item top (${itemTop}) is above header bottom (${headerBottom}): ` +
      `the item is clipped under the header. header height=${headerHeight}px, ` +
      `--mobile-header-height=${cssVar} (${cssVarPx}px). ` +
      `Either useHideOnScroll's ResizeObserver missed the padding change ` +
      `(needs box: 'border-box') or the CSS fallback is too short.`,
    ).toBeGreaterThanOrEqual(headerBottom - 1);
  });

  test('filter panel heading starts below the header', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'threads');

    await waitForVisibleElement(page, 'button[aria-label="Filter threads"]', 10_000);
    const opened = await clickVisibleElement(page, 'button[aria-label="Filter threads"]');
    expect(opened, 'Filter threads button not visible in the mobile threads header').toBe(true);

    await page.waitForFunction(() => {
      const row = document.querySelector('.mobile-swipe-pane .thread-filter-cover[data-open] .thread-filter-panel > :first-child');
      return !!row && row.getBoundingClientRect().height > 0;
    }, undefined, { timeout: 10_000 });

    const result = await page.evaluate(() => {
      const header = document.querySelector('.app-header');
      const panel = document.querySelector('.mobile-swipe-pane .thread-filter-cover');
      const firstRow = document.querySelector('.mobile-swipe-pane .thread-filter-panel > :first-child');
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

    expect(panelScrollTop).toBe(0);

    expect(
      rowTop,
      `first panel row ("${rowLabel}") top (${rowTop}) is above header bottom ` +
      `(${headerBottom}), so the row is hidden under the fixed header. The panel ` +
      `is a scroll container of its own and must carry the ::before header ` +
      `spacer (mobile.css, the .mobile-swipe-pane …::before group).`,
    ).toBeGreaterThanOrEqual(headerBottom - 1);
  });
});
