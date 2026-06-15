/**
 * The "Threads" title in MobileThreadsHeader must start at the same x-position
 * as the "Lucidos" title in MobileThreadHeader, and must not ellipsis-truncate
 * within the constrained mobile header row.
 */
import { test, expect, Page } from '@playwright/test';
import { assertHealthy, navigateToApp, openThreadDrawer } from './helpers';

interface TitleMetrics { left: number; clientWidth: number; scrollWidth: number }

async function getTitleMetrics(page: Page, headerSelector: string, text: string): Promise<TitleMetrics | null> {
  return page.evaluate(({ sel, txt }) => {
    const els = document.querySelectorAll(`${sel} .pane-header-title`);
    for (const el of els) {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && (el.textContent ?? '').trim() === txt) {
        const html = el as HTMLElement;
        return { left: rect.left, clientWidth: html.clientWidth, scrollWidth: html.scrollWidth };
      }
    }
    return null;
  }, { sel: headerSelector, txt: text });
}

test.describe('Mobile threads title alignment', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('Threads title aligns with Lucidos title and is not truncated', async ({ page }) => {
    await navigateToApp(page);

    const lucidos = await getTitleMetrics(page, '.mobile-thread-header', 'Lucidos');
    expect(lucidos, 'Lucidos title not found').not.toBeNull();

    await openThreadDrawer(page);
    const threads = await getTitleMetrics(page, '.mobile-threads-header', 'Threads');
    expect(threads, 'Threads title not found').not.toBeNull();

    // Subpixel rounding tolerance.
    expect(Math.abs(threads!.left - lucidos!.left),
      `Threads title left=${threads!.left} vs Lucidos title left=${lucidos!.left}`)
      .toBeLessThan(1);

    // clientWidth < scrollWidth would mean the title is ellipsis-truncated.
    expect(threads!.clientWidth,
      `Threads title is truncated (clientWidth=${threads!.clientWidth} < scrollWidth=${threads!.scrollWidth})`)
      .toBeGreaterThanOrEqual(threads!.scrollWidth);
  });
});
