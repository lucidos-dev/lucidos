/**
 * Mobile: Settings > Appearance holds still while the theme gallery loads.
 * The picker keeps its height before the skeleton is due, under the skeleton,
 * and once the cards land. The rows below it never move.
 *
 * A phone opens the page cold on almost every visit, over a slower link. So
 * this is where a load slower than the skeleton's delay shows.
 *
 * The gallery includes a single-mode workspace theme. Its card carries the
 * tallest meta row a phone draws: the Custom badge wrapped above its mode.
 */
import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, isMobileViewport, navigateToApp, openSettingsView, waitForEventStream } from './helpers';

// The service worker answers /api/v1 reads itself, where page.route cannot
// hold them.
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

  // Hold the gallery's two reads until the test lets them go.
  let release!: () => void;
  const released = new Promise<void>(resolve => { release = resolve; });
  await page.route(/\/api\/v1\/themes(\/tokens)?(\?|$)/, async route => {
    await released;
    await route.continue();
  });

  await openSettingsView(page, 'appearance');
  await expect(page.locator('[data-search-anchor="appearance:theme-effects"]')).toBeVisible();
  // The gap between the rows either side of the picker is its height.
  // Measured between siblings, so the pane's own swipe cannot move it.
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
