/**
 * Mobile header titles are TRUE-centered on the row middle (like the desktop
 * header and the pane dots), NOT centered between the leading/trailing icon
 * clusters — an in-flow between-clusters title drifts off the row middle whenever
 * the two clusters differ in width, which reads as "off-center".
 *
 * They must also never overlap EITHER icon cluster: a title wide enough to reach
 * one clamps + ellipsizes so its edges stay clear (guaranteed structurally by
 * the absolute centering + a symmetric max-width reserve; the brand's long
 * workspace name truncates within the centered box). This guards both the
 * off-center regression (flanking-spacer centering) and the original overlap
 * regression (a title spilling over the leading icons).
 *
 * Both edges are asserted because the reserve is SYMMETRIC: it is sized off the
 * wider cluster, so a button added to one side spends the other side's
 * clearance. The thread header's trailing cluster grew to compose + search +
 * menu when the hamburger moved to that edge, which is what makes the right-edge
 * assertion load-bearing rather than decorative.
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer } from './helpers';

interface Metrics {
  center: number;
  rowCenter: number;
  left: number;
  right: number;
  leadingRight: number;
  trailingLeft: number;
  clientWidth: number;
  scrollWidth: number;
}

/** Measure a header's centered title against its own row and BOTH icon
 *  clusters. `leadingSel` is the rightmost in-flow leading element,
 *  `trailingSel` the leftmost trailing one. Both edges matter because the
 *  reserve is symmetric: it is sized off whichever cluster is wider, so a button
 *  added to either side eats the other side's clearance too. Returns null if the
 *  header, its row, the title, or either cluster element isn't visible. */
async function measure(
  page: Page,
  headerSel: string,
  titleSel: string,
  leadingSel: string,
  trailingSel: string,
  text?: string,
): Promise<Metrics | null> {
  return page.evaluate(({ headerSel, titleSel, leadingSel, trailingSel, text }) => {
    const header = document.querySelector(headerSel) as HTMLElement | null;
    if (!header || header.getBoundingClientRect().width === 0) return null;
    const row = header.querySelector('.mobile-header-row') as HTMLElement | null;
    const leading = header.querySelector(leadingSel) as HTMLElement | null;
    const trailing = header.querySelector(trailingSel) as HTMLElement | null;
    if (!row || !leading || leading.getBoundingClientRect().width === 0) return null;
    if (!trailing || trailing.getBoundingClientRect().width === 0) return null;
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
        right: rect.right,
        leadingRight: leading.getBoundingClientRect().right,
        trailingLeft: trailing.getBoundingClientRect().left,
        clientWidth: h.clientWidth,
        scrollWidth: h.scrollWidth,
      };
    }
    return null;
  }, { headerSel, titleSel, leadingSel, trailingSel, text });
}

test.describe('Mobile header titles are centered and clear of the leading icons', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('titles sit on the row middle and never overlap the leading cluster', async ({ page }) => {
    await navigateToApp(page);

    // Thread (brand) header: the brand is centered on the row middle and clears
    // BOTH clusters. Leading is the thread-drawer toggle + nav; trailing is
    // compose + search + the menu hamburger, which the hamburger's move to that
    // edge left with the same one-gap clearance the leading side has.
    const brand = await measure(
      page, '.mobile-thread-header', '.pane-header-brand', '.mobile-nav-slot', '.brand-compose-btn',
    );
    expect(brand, 'brand / nav slot / compose button not found').not.toBeNull();
    expect(
      Math.abs(brand!.center - brand!.rowCenter),
      `brand center=${brand!.center} vs row center=${brand!.rowCenter}`,
    ).toBeLessThan(2.5);
    expect(
      brand!.left,
      `brand left=${brand!.left} overlaps nav right=${brand!.leadingRight}`,
    ).toBeGreaterThanOrEqual(brand!.leadingRight - 0.5);
    expect(
      brand!.right,
      `brand right=${brand!.right} overlaps trailing cluster left=${brand!.trailingLeft}`,
    ).toBeLessThanOrEqual(brand!.trailingLeft + 0.5);

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
    const threads = await measure(
      page, '.mobile-threads-header', '.pane-header-title', '.view-selector-slot', '.brand-compose-btn', 'Threads',
    );
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
      threads!.right,
      `Threads title right=${threads!.right} overlaps trailing cluster left=${threads!.trailingLeft}`,
    ).toBeLessThanOrEqual(threads!.trailingLeft + 0.5);
    expect(
      threads!.clientWidth,
      `Threads title truncated (clientWidth=${threads!.clientWidth} < scrollWidth=${threads!.scrollWidth})`,
    ).toBeGreaterThanOrEqual(threads!.scrollWidth);
  });
});
