import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';


test('a short desktop window keeps the desktop split', async ({ page }) => {
  await assertHealthy(page);
  await navigateToApp(page);
  await page.setViewportSize({ width: 1000, height: 450 });
  await page.waitForFunction(() => window.innerHeight === 450);
  const layout = await page.evaluate(() => ({
    phone: !!document.querySelector('.mobile-swipe-wrapper'),
    split: !!document.querySelector('.split-layout'),
  }));
  expect(layout).toEqual({ phone: false, split: true });
});
