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

/** A viewport that leaves the phone layout swaps App to the desktop layout and
 *  back. A phone in landscape keeps the phone layout (ADR 0342), so a tablet's
 *  size stands in for the swap. The header outlives it, and the bars hook it
 *  carries must bind the NEW mobile tree's title bar, prompt and chevrons. Bound to the old ones, the keyboard left the title bar on
 *  screen and the down chevron fell onto Send. `hooks/useHideOnScroll.ts`. */

const CONTAINER = '.mobile-swipe-pane .thread-content.visible';

// Wide and tall, so the desktop split mounts even under a finger.
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

    // A transcript tall enough to scroll, read from its middle, so the down
    // chevron is up.
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
    // Polled, since the prompt and the chevron glide back in from away.
    await expect.poll(async () => {
      const chevron = await box(page, '.mobile-swipe-pane .scroll-to-bottom');
      const prompt = await box(page, '.mobile-swipe-pane .prompt-area .prompt-box');
      return prompt!.top - chevron!.bottom;
    }).toBeGreaterThanOrEqual(0);
  });
});
