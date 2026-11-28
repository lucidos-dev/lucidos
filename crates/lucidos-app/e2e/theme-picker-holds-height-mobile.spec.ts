import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, isMobileViewport, navigateToApp, openSettingsView, waitForEventStream } from './helpers';

test.use({ serviceWorkers: 'block' });

const CUSTOM_THEME = 'e2e-picker-height';

test.afterEach(async ({ page }) => {
  await apiRequest(page).delete(`/api/v1/data/themes/${CUSTOM_THEME}.json`);
});

test('the theme picker holds its height while the gallery loads', async ({ page }) => {
  test.skip(!isMobileViewport(page), 'Phone layout only, so the desktop project skips it');
  await assertHealthy(page);
  const put = await apiRequest(page).put(`/api/v1/data/themes/${CUSTOM_THEME}.json`, {
    data: JSON.stringify({ name: 'Picker Height', dark: { '--accent': '#d4a650' } }),
  });
  expect(put.ok(), `PUT the workspace theme -> ${put.status()}`).toBeTruthy();
  await navigateToApp(page);
  await waitForEventStream(page);

  let release!: () => void;
  const released = new Promise<void>(resolve => { release = resolve; });
  await page.route(/\/api\/v1\/themes(\/tokens)?(\?|$)/, async route => {
    await released;
    await route.continue();
  });

  await openSettingsView(page, 'appearance');
  await expect(page.locator('[data-search-anchor="appearance:theme-effects"]')).toBeVisible();
  const gap = () => page.evaluate(() => {
    const above = document.querySelector('[data-search-anchor="appearance:mode"]')!.getBoundingClientRect();
    const below = document.querySelector('[data-search-anchor="appearance:theme-effects"]')!.getBoundingClientRect();
    return Math.round(below.top - above.bottom);
  });

  const beforeSkeleton = await gap();
  expect(beforeSkeleton, 'the picker drew no box before its skeleton').toBeGreaterThan(100);

  await expect(page.locator('.loading-fade-skeleton .theme-carousel')).toBeVisible();
  expect(await gap(), 'the skeleton moved the rows below it').toBe(beforeSkeleton);

  release();
  await expect(page.locator('.loading-fade-content .theme-card[role="radio"]').first()).toBeVisible();
  expect(await gap(), 'the loaded cards moved the rows below them').toBe(beforeSkeleton);
});
