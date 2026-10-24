/**
 * A Plugins panel row leads with its plugin icon and opens the plugin detail
 * page (ADR 0414, Phase 4). The page shows every screenshot and video in one
 * strip, the README, and the media the engine left out.
 *
 * The catalog read and the media route are mocked, so one row carries media
 * without a marketplace. The route's own headers are pinned in the API e2e
 * suite (`plugin_catalog_test.rs`).
 */
import { test, expect, type Page } from './fixtures';
import { gotoWithRetry } from './helpers';

// The page reads the catalog more than once, and a read the service worker
// makes skips the route mocks below.
test.use({ serviceWorkers: 'block' });

const MEDIA = '/api/v1/plugins/media/catalog/mkt-e2e/habit-tracker/0.1.0';

/** A 1x1 PNG, enough for an `<img>` to load. */
const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==',
  'base64',
);

const PLUGIN = {
  marketplace_id: 'mkt-e2e',
  marketplace_name: 'E2E Marketplace',
  id: 'habit-tracker',
  name: 'Habit Tracker',
  description: 'Track one habit a day.',
  version: '0.1.0',
  source: 'https://example.com/habit-tracker',
  manifest: {},
  content: ['apps'],
  categories: [],
  files_count: 3,
  status: 'available',
  engine_requirement: '>=0.1.0',
  engine_compatible: true,
  media: {
    icon_url: `${MEDIA}/media/icon.svg`,
    screenshots: [`${MEDIA}/media/one.png`, `${MEDIA}/media/two.png`],
    videos: [],
    readme_url: `${MEDIA}/media/README.md`,
    problems: [{ path: 'media/huge.png', reason: 'is 5.0 MB, over the 4.0 MB screenshot limit' }],
  },
};

async function openPlugins(page: Page) {
  await page.route('**/api/v1/plugins/catalog', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        marketplaces: [{ id: 'mkt-e2e', name: 'E2E Marketplace', source: PLUGIN.source }],
        plugins: [PLUGIN],
        errors: [],
        scanned_at: null,
        scanning: false,
        scan_error: null,
      }),
    }));
  await page.route('**/api/v1/plugins/media/**', (route) => {
    const url = route.request().url();
    if (url.endsWith('.svg')) {
      return route.fulfill({ status: 200, contentType: 'image/svg+xml', body: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"><rect width="1" height="1"/></svg>' });
    }
    if (url.endsWith('.md')) {
      return route.fulfill({ status: 200, contentType: 'text/plain', body: '# Track a habit\n\nOne tap a day.<script>window.hacked = 1</script>' });
    }
    return route.fulfill({ status: 200, contentType: 'image/png', body: PNG });
  });
  await page.addInitScript(() => {
    localStorage.setItem('lucidos-active-menu-item', 'plugins');
    localStorage.setItem('lucidos-plugins-installed-only', 'false');
  });
  await gotoWithRetry(page, '/');
  await expect(page.locator('.boot-splash')).toHaveCount(0, { timeout: 30_000 });
}

test('a plugin row opens its detail page with every screenshot and the README', async ({ page }) => {
  await openPlugins(page);
  const row = page.locator('.app-store-plugin-row[data-plugin-id="habit-tracker"]:visible');
  await expect(row).toBeVisible({ timeout: 15_000 });
  await expect(row.locator('.app-store-plugin-icon .app-icon-image img')).toBeVisible();
  await expect(row.locator('img')).toHaveCount(1);

  await row.locator('.app-store-plugin-description').click();
  const detail = page.locator('.plugin-detail:visible');
  await expect(detail.locator('.plugin-detail-name')).toHaveText('Habit Tracker');
  await expect(detail.locator('.plugin-detail-shot')).toHaveCount(2);
  await expect(detail.locator('.plugin-detail-readme h1')).toHaveText('Track a habit');
  expect(await page.evaluate(() => (window as unknown as { hacked?: number }).hacked)).toBeUndefined();
  await expect(detail.locator('.plugin-detail-problems')).toContainText('media/huge.png');
  await expect(detail.locator('iframe')).toHaveCount(0);

  await detail.locator('.plugin-detail-shot').nth(1).click();
  await expect(page.locator('.image-popup-content img').first()).toBeVisible();
  await expect(page.locator('.image-popup-nav-prev')).toBeVisible();
});
