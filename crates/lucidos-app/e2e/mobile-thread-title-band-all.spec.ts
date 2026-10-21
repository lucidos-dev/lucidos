/**
 * The mobile title row sits on the page while the transcript rests at its top.
 * Its --bg-secondary band fades in once content scrolls under it. It draws no
 * hairline in either state.
 *
 * The band is what lets the title leave with the header as one bar. Without
 * it the title drifts away like loose transcript text.
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

/** The visible mobile title row's background, the two theme colours it moves
 *  between, and whether it draws a `::before` line. */
async function readBand(page: import('@playwright/test').Page) {
  return page.evaluate(() => {
    const probe = (token: string) => {
      const el = document.createElement('div');
      el.style.background = `var(${token})`;
      document.body.appendChild(el);
      const color = getComputedStyle(el).backgroundColor;
      el.remove();
      return color;
    };
    const row = Array.from(document.querySelectorAll<HTMLElement>('.mobile-thread-title-row'))
      .find((r) => r.getBoundingClientRect().width > 0);
    if (!row) return null;
    return {
      background: getComputedStyle(row).backgroundColor,
      hairline: getComputedStyle(row, '::before').content,
      page: probe('--bg-primary'),
      band: probe('--bg-secondary'),
    };
  });
}

test.describe('Mobile thread title band', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('rests on the page with no hairline, and bands once scrolled', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('band');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    // A one-turn thread does not scroll, so the row rests unscrolled.
    const rest = await readBand(page);
    expect(rest, 'visible .mobile-thread-title-row not found').not.toBeNull();
    expect(rest!.background).toBe(rest!.page);
    expect(rest!.hairline).toBe('none');

    // The class is what scrolling sets (scrolledFromTop). The band then fades
    // in over --duration-normal, so poll until it lands.
    await page.evaluate(() => {
      document.querySelectorAll('.mobile-thread-title-row').forEach((r) => r.classList.add('scrolled'));
    });
    await expect.poll(async () => {
      const b = await readBand(page);
      return b && b.background === b.band;
    }).toBe(true);
    expect((await readBand(page))!.hairline).toBe('none');
  });
});
