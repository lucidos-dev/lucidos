import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  waitForResponse,
  uniqueMessage,
  blurActiveElement,
  waitForThreadTitle,
  getVisibleTitleText,
  getMobileTitleHeight,
  enableMobileDynamicBars,
  disarmFollowSeed,
  renameThreadViaMenu,
} from './helpers';


async function newThread(page: import('@playwright/test').Page, tag: string): Promise<void> {
  await navigateToApp(page);
  await sendMessage(page, `Say exactly: "${uniqueMessage(tag)}"`);
  await waitForResponse(page);
  await waitForThreadTitle(page);
}

test.describe('Thread title and rename: mobile', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('shows the title in the mobile title row and renames through its ⋯ menu', async ({ page }) => {
    await newThread(page, 'mobile-rename');
    expect(await getMobileTitleHeight(page)).toBeGreaterThan(10);
    await expect(page.locator('.mobile-thread-title-row input, .mobile-thread-title-row textarea')).toHaveCount(0);

    const next = `Mobile rename ${Date.now()}`;
    await renameThreadViaMenu(page, next);
    await expect.poll(() => getVisibleTitleText(page)).toBe(next);
    expect(await getMobileTitleHeight(page), 'title must not collapse after a rename').toBeGreaterThan(10);
  });

  test('title hides with header on scroll down', async ({ page }) => {
    await disarmFollowSeed(page);
    await enableMobileDynamicBars(page);
    await navigateToApp(page);

    const msg = uniqueMessage('scroll-title');
    await sendMessage(page, `Say exactly: "${msg}" and then write a very long paragraph with at least 200 words about anything`);
    await waitForResponse(page);

    await waitForThreadTitle(page);

    const titleVisibleBefore = await page.evaluate(() => {
      const els = document.querySelectorAll('.mobile-thread-title-row .thread-title');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    });
    expect(titleVisibleBefore).toBe(true);

    await blurActiveElement(page);

    await page.evaluate(() => {
      const container = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      if (container) {
        const filler = document.createElement('div');
        filler.style.minHeight = '2000px';
        filler.style.flexShrink = '0';
        filler.dataset.testFiller = 'true';
        container.appendChild(filler);
      }
    });

    await page.waitForFunction(() => {
      const container = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      return container ? container.scrollHeight > container.clientHeight : false;
    }, undefined, { timeout: 5_000 });

    await page.evaluate(() => {
      const container = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      if (container) container.scrollTop = 0;
    });
    await page.waitForFunction(() => {
      const container = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      const header = document.querySelector('.app-header');
      return !!container && container.scrollTop === 0
        && !!header && header.getBoundingClientRect().bottom > 0;
    }, undefined, { timeout: 5_000 });

    await page.evaluate(async () => {
      const container = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      if (!container) return;
      const step = Math.max(80, Math.floor(container.clientHeight / 2));
      for (let i = 0; i < 40; i++) {
        const before = container.scrollTop;
        container.scrollTop = before + step;
        if (container.scrollTop === before) break;
        await new Promise(r => setTimeout(r, 80));
      }
    });

    await page.waitForFunction(() => {
      const header = document.querySelector('.app-header');
      if (!header) return false;
      return header.getBoundingClientRect().bottom <= 0;
    }, undefined, { timeout: 5_000 });

    await expect.poll(() => page.evaluate(() => {
      const els = document.querySelectorAll('.mobile-thread-title-row');
      for (const el of els) {
        const rect = el.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          return rect.bottom <= 0;
        }
      }
      return true;
    }), { timeout: 5_000 }).toBe(true);
  });
});
