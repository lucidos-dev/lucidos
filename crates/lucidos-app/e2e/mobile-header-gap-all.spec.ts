import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, sendMessage, uniqueMessage, waitForResponse, ensureOnThreadPane } from './helpers';

test.describe('Mobile header gap regression', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('--mobile-header-height matches actual header height (subpixel)', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('header-gap');
    await sendMessage(page, `Say exactly: "gap test ${msg}"`);
    await waitForResponse(page);
    await ensureOnThreadPane(page);

    await page.waitForFunction(() =>
      getComputedStyle(document.documentElement).getPropertyValue('--mobile-header-height').trim() !== ''
    , { timeout: 5_000 });

    const result = await page.evaluate(() => {
      const header = document.querySelector('.app-header');
      if (!header) return { error: 'no header' };
      const actualHeight = header.getBoundingClientRect().height;
      const cssVar = getComputedStyle(document.documentElement).getPropertyValue('--mobile-header-height').trim();
      if (!cssVar) return { error: 'no CSS var' };
      const remSize = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
      const cssVarPx = parseFloat(cssVar) * remSize;
      return { actualHeight, cssVarPx, diff: Math.abs(actualHeight - cssVarPx) };
    });

    expect(result).not.toHaveProperty('error');
    const { actualHeight, cssVarPx, diff } = result as { actualHeight: number; cssVarPx: number; diff: number };

    expect(diff).toBeLessThan(0.5);
    expect(actualHeight).toBeGreaterThan(0);
    expect(cssVarPx).toBeGreaterThan(0);
  });

  test('resume reconciles a missed header resize while scrolled', async ({ page }) => {
    await page.addInitScript(() => {
      const Observer = window.ResizeObserver;
      window.ResizeObserver = class extends Observer {
        constructor(callback: ResizeObserverCallback) {
          super((entries, observer) => {
            const others = entries.filter(entry => !entry.target.matches('.app-header'));
            if (others.length) callback(others, observer);
          });
        }
      };
    });
    await navigateToApp(page);
    await sendMessage(page, `Say exactly: "${uniqueMessage('resume-gap')}"`);
    await waitForResponse(page);
    await ensureOnThreadPane(page);
    await page.waitForTimeout(700);
    await page.evaluate(() => {
      const header = document.querySelector<HTMLElement>('.app-header')!;
      header.style.paddingBottom = '100px';
      window.dispatchEvent(new Event('resize'));
      const scroller = document.querySelector<HTMLElement>('.mobile-swipe-pane .thread-content.visible')!;
      const filler = document.createElement('div');
      filler.dataset.resumeAnchor = '';
      filler.style.height = '2000px';
      scroller.append(filler);
      scroller.scrollTop = 300;
    });
    await page.waitForTimeout(100);
    const anchor = page.locator('[data-resume-anchor]');
    const before = await anchor.evaluate(el => el.getBoundingClientRect().top);
    await page.evaluate(() => {
      document.querySelector<HTMLElement>('.app-header')!.style.removeProperty('padding-bottom');
      window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await expect.poll(() => page.evaluate(() => {
      const header = document.querySelector('.app-header')!.getBoundingClientRect();
      const title = document.querySelector('.mobile-swipe-pane .mobile-thread-title-row')!.getBoundingClientRect();
      return Math.abs(title.top - header.bottom);
    })).toBeLessThan(1);
    expect(await anchor.evaluate(el => el.getBoundingClientRect().top)).toBeCloseTo(before, 0);
  });

  test('thread title bar sits flush against header bottom when sticky', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('flush-check');
    await sendMessage(page, `Repeat the word "test" 50 times, each on its own line. Start with: ${msg}`);
    await waitForResponse(page);
    await ensureOnThreadPane(page);

    await page.waitForFunction(() =>
      getComputedStyle(document.documentElement).getPropertyValue('--mobile-header-height').trim() !== ''
    , { timeout: 5_000 });

    await page.evaluate(() => {
      const tc = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      if (tc) tc.scrollTop = 200;
    });

    await page.waitForTimeout(100);

    const result = await page.evaluate(() => {
      const header = document.querySelector('.app-header');
      const titleRow = document.querySelector('.mobile-swipe-pane .mobile-thread-title-row');
      if (!header || !titleRow) return { error: 'missing elements' };
      const headerBottom = header.getBoundingClientRect().bottom;
      const titleTop = titleRow.getBoundingClientRect().top;
      return { gap: titleTop - headerBottom };
    });

    expect(result).not.toHaveProperty('error');
    const { gap } = result as { gap: number };

    expect(gap).toBeLessThan(1);
  });
});
