import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  waitForResponse,
  uniqueMessage,
  waitForThreadTitle,
} from './helpers';

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

    const rest = await readBand(page);
    expect(rest, 'visible .mobile-thread-title-row not found').not.toBeNull();
    expect(rest!.background).toBe(rest!.page);
    expect(rest!.hairline).toBe('none');

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
