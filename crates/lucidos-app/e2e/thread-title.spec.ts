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

/** The mobile title row: display-only, renamed through its ⋯ menu, and
 *  hiding with the header on scroll. Desktop rename lives in
 *  thread-title-rename-desktop.spec.ts. */

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
    // The follow seed ships ARMED, and a rider is carried back to the live edge
    // by any scroll that is not their own gesture. This test parks the reader at
    // the top and drags down from there, so it starts them disarmed.
    await disarmFollowSeed(page);
    // Dynamic bars default OFF, which pins the header. This test asserts the
    // header (and sticky title bar) scroll off, so turn them on before the page
    // boots.
    await enableMobileDynamicBars(page);
    await navigateToApp(page);

    // Send multiple messages to create scrollable content
    const msg = uniqueMessage('scroll-title');
    await sendMessage(page, `Say exactly: "${msg}" and then write a very long paragraph with at least 200 words about anything`);
    await waitForResponse(page);

    await waitForThreadTitle(page);

    // Verify title is visible initially
    const titleVisibleBefore = await page.evaluate(() => {
      const els = document.querySelectorAll('.mobile-thread-title-row .thread-title');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    });
    expect(titleVisibleBefore).toBe(true);

    // Blur any focused input: useHideOnScroll skips the header hide while a
    // text input in the same pane has focus, so typing never hides it.
    await blurActiveElement(page);

    // Inject extra content to guarantee the container is scrollable, since LLM
    // output length is unpredictable and may be too short on a small viewport.
    await page.evaluate(() => {
      const container = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      if (container) {
        const filler = document.createElement('div');
        // min-height with flex-shrink:0 keeps the flex container from
        // collapsing the filler. A plain height shrinks to fit the layout.
        filler.style.minHeight = '2000px';
        filler.style.flexShrink = '0';
        filler.dataset.testFiller = 'true';
        container.appendChild(filler);
      }
    });

    // Wait until the container is scrollable (filler appended + layout settled)
    await page.waitForFunction(() => {
      const container = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      return container ? container.scrollHeight > container.clientHeight : false;
    }, undefined, { timeout: 5_000 });

    // Park the reader at the top FIRST, so what follows is a real scroll down.
    // The filler above grew the transcript, and growth leaves the reader at the
    // bottom. A scroll to where we already are gives hide-on-scroll no delta to
    // act on. This write sticks only because the test disarmed the follow. An
    // armed follow answers growth by writing the reader back to the live edge,
    // and only a reader GESTURE retires it (scrollState.ts `onScroll`).
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

    // Scroll down the way a drag does, in spaced steps, NOT in one write.
    // `useHideOnScroll` skips any scroll event inside the app's own 64ms
    // navigation-scroll window (`isNavigationScroll`), and landing at the top
    // can open that window. One jump to the bottom is ONE event. If it lands
    // inside the window, it is swallowed and no later event comes, so the
    // header stays up at the live edge. A real drag emits events across
    // hundreds of ms, and each step here is spaced past the window likewise.
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

    // Wait for header to hide (translateY should be negative)
    await page.waitForFunction(() => {
      const header = document.querySelector('.app-header');
      if (!header) return false;
      return header.getBoundingClientRect().bottom <= 0;
    }, undefined, { timeout: 5_000 });

    // The title bar is sticky inside the scroll container and scrolls out
    // together with the header. Verify it is off-screen after full scroll.
    const titleOffScreen = await page.evaluate(() => {
      const els = document.querySelectorAll('.mobile-thread-title-row');
      for (const el of els) {
        const rect = el.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          return rect.bottom <= 0;
        }
      }
      return true; // not rendered = off-screen
    });
    expect(titleOffScreen).toBe(true);
  });
});
