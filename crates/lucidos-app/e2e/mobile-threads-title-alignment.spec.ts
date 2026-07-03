/**
 * Mobile header titles are TRUE-centered on the row middle (like the desktop
 * header and the pane dots), NOT centered between the leading/trailing icon
 * clusters — an in-flow between-clusters title drifts off the row middle whenever
 * the two clusters differ in width, which reads as "off-center".
 *
 * They must also never overlap the leading icon cluster: a title wide enough to
 * reach it clamps + ellipsizes so its left edge stays past the icons (guaranteed
 * structurally by the absolute centering + a symmetric max-width reserve; the
 * brand's long workspace name truncates within the centered box). This guards
 * both the off-center regression (flanking-spacer centering) and the original
 * overlap regression (a title spilling over the hamburger/nav).
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer } from './helpers';

interface Metrics {
  center: number;
  rowCenter: number;
  left: number;
  leadingRight: number;
  clientWidth: number;
  scrollWidth: number;
}

/** Measure a header's centered title against its own row and its leading icon
 *  cluster. `leadingSel` is the rightmost in-flow leading element. Returns null
 *  if the header, its row, the title, or the leading element isn't visible. */
async function measure(
  page: Page,
  headerSel: string,
  titleSel: string,
  leadingSel: string,
  text?: string,
): Promise<Metrics | null> {
  return page.evaluate(({ headerSel, titleSel, leadingSel, text }) => {
    const header = document.querySelector(headerSel) as HTMLElement | null;
    if (!header || header.getBoundingClientRect().width === 0) return null;
    const row = header.querySelector('.mobile-header-row') as HTMLElement | null;
    const leading = header.querySelector(leadingSel) as HTMLElement | null;
    if (!row || !leading || leading.getBoundingClientRect().width === 0) return null;
    const rowRect = row.getBoundingClientRect();
    for (const el of header.querySelectorAll(titleSel)) {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0) continue;
      if (text != null && (el.textContent ?? '').trim() !== text) continue;
      const h = el as HTMLElement;
      return {
        center: rect.left + rect.width / 2,
        rowCenter: rowRect.left + rowRect.width / 2,
        left: rect.left,
        leadingRight: leading.getBoundingClientRect().right,
        clientWidth: h.clientWidth,
        scrollWidth: h.scrollWidth,
      };
    }
    return null;
  }, { headerSel, titleSel, leadingSel, text });
}

test.describe('Mobile header titles are centered and clear of the leading icons', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('titles sit on the row middle and never overlap the leading cluster', async ({ page }) => {
    await navigateToApp(page);

    // Thread (brand) header: the brand is centered on the row middle and its left
    // edge stays past the hamburger + nav cluster.
    const brand = await measure(page, '.mobile-thread-header', '.pane-header-brand', '.mobile-nav-slot');
    expect(brand, 'brand / nav slot not found').not.toBeNull();
    expect(
      Math.abs(brand!.center - brand!.rowCenter),
      `brand center=${brand!.center} vs row center=${brand!.rowCenter}`,
    ).toBeLessThan(2.5);
    expect(
      brand!.left,
      `brand left=${brand!.left} overlaps nav right=${brand!.leadingRight}`,
    ).toBeGreaterThanOrEqual(brand!.leadingRight - 0.5);

    // The workspace name must stay VISIBLE in the brand — the absolute-centered
    // brand is shrink-to-content, so the name-hide budget must include the
    // trailing spacer's slack; dropping it latched the name hidden (regression).
    // (Skips only if this workspace has no name to render.)
    const wsName = await page.evaluate(() => {
      const el = document.querySelector('.mobile-thread-header .workspace-name-label') as HTMLElement | null;
      if (!el) return null;
      return { hidden: el.classList.contains('is-hidden'), clientWidth: el.clientWidth };
    });
    if (wsName) {
      expect(wsName.hidden, 'workspace name is .is-hidden in the brand').toBe(false);
      expect(wsName.clientWidth, 'workspace name has zero width in the brand').toBeGreaterThan(0);
    }

    // Threads header: "Threads" is centered on the row middle, not truncated, and
    // clear of the Filter control.
    await openThreadDrawer(page);
    const threads = await measure(page, '.mobile-threads-header', '.pane-header-title', '.view-selector-slot', 'Threads');
    expect(threads, 'Threads title / filter slot not found').not.toBeNull();
    expect(
      Math.abs(threads!.center - threads!.rowCenter),
      `Threads center=${threads!.center} vs row center=${threads!.rowCenter}`,
    ).toBeLessThan(2.5);
    expect(
      threads!.left,
      `Threads title left=${threads!.left} overlaps filter right=${threads!.leadingRight}`,
    ).toBeGreaterThanOrEqual(threads!.leadingRight - 0.5);
    expect(
      threads!.clientWidth,
      `Threads title truncated (clientWidth=${threads!.clientWidth} < scrollWidth=${threads!.scrollWidth})`,
    ).toBeGreaterThanOrEqual(threads!.scrollWidth);
  });
});
