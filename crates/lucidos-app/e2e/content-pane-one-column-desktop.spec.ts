/**
 * Every content-pane view, tab or overlay, renders as one column: the same
 * left edge, width and top as the Apps view. The plugin detail page and the
 * inline forms once rendered full width and left-aligned. The column was
 * opt-in, and only the tab views opted in.
 *
 * The source scan `styles/__tests__/content-pane-one-column.test.ts` pins the
 * rule. This measures what renders, on a pane wide enough for the cap to bite.
 */
import { test, expect, type Page } from './fixtures';
import { addTriggerCard, apiRequest, assertHealthy, navigateToApp, openSettingsView, waitForEventStream } from './helpers';

// The plugin catalog read is mocked, and a read the service worker makes
// skips the route mock.
test.use({ serviceWorkers: 'block', viewport: { width: 2560, height: 900 } });

interface Column {
  left: number;
  width: number;
  /** Where the first content can start: the view's top plus its top padding. */
  top: number;
  paneWidth: number;
}

async function measure(page: Page): Promise<Column> {
  return page.locator('.content-pane-body:visible').first().evaluate((pane) => {
    const view = pane.firstElementChild as HTMLElement;
    const r = view.getBoundingClientRect();
    const p = pane.getBoundingClientRect();
    return {
      left: r.left - p.left,
      width: r.width,
      top: r.top - p.top + parseFloat(getComputedStyle(view).paddingTop),
      paneWidth: pane.clientWidth,
    };
  });
}

async function navigate(page: Page, target: string): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target, params: {} },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate ${target} -> ${res.status()}`).toBeTruthy();
}

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
  media: { screenshots: [], videos: [], problems: [] },
};

test('every content-pane view shares the Apps column', async ({ page }) => {
  await page.route('**/api/v1/plugins/catalog', (route) => route.fulfill({
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
  await assertHealthy(page);
  await navigateToApp(page);
  await waitForEventStream(page);

  await navigate(page, 'apps');
  await expect(page.locator('.content-pane-body .apps-view:visible')).toBeVisible({ timeout: 15_000 });
  const apps = await measure(page);
  expect(apps.width, 'the pane is too narrow for the cap, so this proves nothing')
    .toBeLessThan(apps.paneWidth - 50);

  const views: Array<[string, () => Promise<void>]> = [
    ['plugins', async () => {
      await navigate(page, 'plugins');
      await expect(page.locator('.content-pane-body .plugins-view:visible')).toBeVisible({ timeout: 15_000 });
    }],
    ['plugin detail', async () => {
      await page.locator('.content-pane-body .segmented-control:visible button', { hasText: 'All' }).first().click();
      await page.locator('.app-store-plugin-row[data-plugin-id="habit-tracker"]:visible .app-store-plugin-description').click();
      await expect(page.locator('.plugin-detail:visible .plugin-detail-name')).toHaveText('Habit Tracker');
    }],
    ['triggers', async () => {
      await navigate(page, 'triggers');
      await expect(addTriggerCard(page)).toBeVisible({ timeout: 15_000 });
    }],
    ['new trigger form', async () => {
      await addTriggerCard(page).click();
      await expect(page.locator('.content-pane-body .inline-form:visible').first()).toBeVisible({ timeout: 10_000 });
    }],
    ['changes', async () => {
      await navigate(page, 'changes');
      await expect(page.locator('.content-pane-body > .panel-content:visible')).toBeVisible({ timeout: 15_000 });
    }],
    ['notifications', async () => {
      await navigate(page, 'notifications');
      await expect(page.locator('.content-pane-body > .panel-content:visible')).toBeVisible({ timeout: 15_000 });
    }],
    ['settings', async () => {
      await openSettingsView(page, 'appearance');
    }],
  ];

  for (const [name, open] of views) {
    await open();
    await expect.poll(async () => {
      const m = await measure(page);
      const off = Math.max(Math.abs(m.left - apps.left), Math.abs(m.width - apps.width), Math.abs(m.top - apps.top));
      return off <= 1 ? 'aligned' : JSON.stringify({ apps, [name]: m });
    }, { timeout: 5_000, message: `${name} is off the Apps column` }).toBe('aligned');
  }
});
