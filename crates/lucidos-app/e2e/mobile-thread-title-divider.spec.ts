/**
 * Regression: the thread title header divider was missing on mobile.
 *
 * On desktop the title bar (`.thread-view-header`) carries a 1px `var(--border-color)`
 * hairline via `::after`. On mobile that element is `display: none` and the title
 * renders in the sticky `.mobile-thread-title-row`, which had no divider — only a
 * scroll-fade `::after` gradient that is `opacity: 0` at rest. So at rest there was
 * no separation between the title header and the transcript.
 *
 * The mobile title row must show the same bottom hairline as desktop, at rest,
 * via a `::before` line (no layout height, so the sticky scroll offset is intact).
 */
import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  waitForResponse,
  uniqueMessage,
  waitForThreadTitle,
} from './helpers';

test.describe('Mobile thread title divider', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('title row shows a bottom hairline divider at rest', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('divider');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    const divider = await page.evaluate(() => {
      const rows = document.querySelectorAll('.mobile-thread-title-row');
      for (const row of rows) {
        const rect = row.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          // The divider lives on ::before, independent of `.scrolled` (which only
          // gates the ::after fade gradient) — so it must render in any scroll
          // state. Before the fix the ::before rule didn't exist, so its computed
          // `content` was `none`.
          const cs = getComputedStyle(row, '::before');
          return {
            content: cs.content,
            position: cs.position,
            height: cs.height,
            background: cs.backgroundColor,
          };
        }
      }
      return null;
    });

    expect(divider, 'visible .mobile-thread-title-row not found').not.toBeNull();
    // ::before must render as a 1px absolutely-positioned line in a real color.
    expect(divider!.content).not.toBe('none');
    expect(divider!.position).toBe('absolute');
    expect(divider!.height).toBe('1px');
    expect(divider!.background, 'divider has no color (transparent)').not.toBe('rgba(0, 0, 0, 0)');
  });
});
