import { test, expect, type Page } from './fixtures';
import {
  assertHealthy,
  blurActiveElement,
  disableMobileDynamicBars,
  disarmFollowSeed,
  enableMobileDynamicBars,
  navigateToApp,
  sendMessage,
  uniqueMessage,
  waitForResponse,
} from './helpers';


const CONTAINER = '.mobile-swipe-pane .thread-content.visible';

const TABLET = { width: 1180, height: 820 };

async function rotate(page: Page): Promise<void> {
  const portrait = page.viewportSize()!;
  await page.setViewportSize(TABLET);
  await expect(page.locator('.mobile-swipe-wrapper')).toHaveCount(0);
  await page.setViewportSize(portrait);
  await expect(page.locator(CONTAINER)).toBeVisible();
}

function box(page: Page, sel: string): Promise<{ top: number; bottom: number } | null> {
  return page.evaluate((s) => {
    const el = document.querySelector(s);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { top: r.top, bottom: r.bottom };
  }, sel);
}

test.describe('Dynamic bars after a rotation', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await disarmFollowSeed(page);
  });

  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
  });

  test('the keyboard sends the title bar away and keeps the down chevron above the prompt', async ({ page }) => {
    await enableMobileDynamicBars(page);
    await navigateToApp(page);
    await sendMessage(page, `Say exactly: "${uniqueMessage('rotation')}"`);
    await waitForResponse(page);
    await blurActiveElement(page);

    await rotate(page);

    await page.evaluate((sel) => {
      const filler = document.createElement('div');
      filler.style.minHeight = '3000px';
      document.querySelector(sel)!.appendChild(filler);
    }, CONTAINER);
    await page.evaluate(async (sel) => {
      const c = document.querySelector(sel)!;
      for (let i = 0; i < 8; i++) {
        c.scrollTop += 80;
        await new Promise((r) => setTimeout(r, 80));
      }
    }, CONTAINER);
    await expect(page.locator('.mobile-swipe-pane .scroll-to-bottom.visible')).toHaveCount(1);

    await page.locator('.mobile-swipe-pane [data-role="prompt-input"]').tap();

    await expect.poll(async () => (await box(page, '.mobile-swipe-pane .mobile-thread-title-row'))?.bottom)
      .toBeLessThanOrEqual(0);
    await expect.poll(async () => {
      const chevron = await box(page, '.mobile-swipe-pane .scroll-to-bottom');
      const prompt = await box(page, '.mobile-swipe-pane .prompt-area .prompt-box');
      return prompt!.top - chevron!.bottom;
    }).toBeGreaterThanOrEqual(0);
  });
});
