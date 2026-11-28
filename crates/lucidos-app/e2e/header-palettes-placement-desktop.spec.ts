import { test, expect, Page } from './fixtures';
import { assertHealthy, clickHeaderAction, navigateToApp, openFilesPanel } from './helpers';

interface Geometry {
  centre: number;
  top: number;
  left: number;
  right: number;
}

async function geometry(page: Page, selector: string): Promise<Geometry> {
  return page.locator(selector).first().evaluate((el) => {
    const r = el.getBoundingClientRect();
    return { centre: (r.left + r.right) / 2, top: r.top, left: r.left, right: r.right };
  });
}

function expectInside(panel: Geometry, pane: Geometry): void {
  expect(panel.left, 'palette starts inside its pane').toBeGreaterThanOrEqual(pane.left - 0.5);
  expect(panel.right, 'palette ends inside its pane').toBeLessThanOrEqual(pane.right + 0.5);
}

async function headerLine(page: Page): Promise<number> {
  await page.locator('.desktop-header [data-role="brand-menu-toggle"]').click();
  const top = (await geometry(page, '.brand-menu')).top;
  await page.keyboard.press('Escape');
  await expect(page.locator('.brand-menu')).toHaveCount(0);
  return top;
}

test.describe('Header palettes: desktop placement', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await page.setViewportSize({ width: 1600, height: 900 });
    await navigateToApp(page);
  });

  test('Search Everywhere hangs on the header line, centred over the Conversation pane', async ({ page }) => {
    const line = await headerLine(page);
    await clickHeaderAction(page, '.search-everywhere-btn');
    await expect(page.locator('.search-everywhere-modal')).toBeVisible();

    const panel = await geometry(page, '.search-everywhere-modal');
    const pane = await geometry(page, '.split-layout > .pane-thread');
    expect(Math.abs(panel.top - line), 'same line as the Lucidos menu').toBeLessThan(1);
    expect(Math.abs(panel.centre - pane.centre), 'centred over the Conversation pane').toBeLessThan(1);
    expectInside(panel, pane);
  });

  test('file search hangs on the header line, centred over the Canvas pane', async ({ page }) => {
    const line = await headerLine(page);
    await openFilesPanel(page);
    await clickHeaderAction(page, '.file-search-btn');
    await expect(page.locator('.file-search-overlay:not(.file-search-closed) .file-search-modal')).toBeVisible();

    const panel = await geometry(page, '.file-search-modal');
    const pane = await geometry(page, '.split-layout > .pane-content');
    expect(Math.abs(panel.top - line), 'same line as the Lucidos menu').toBeLessThan(1);
    expect(Math.abs(panel.centre - pane.centre), 'centred over the Canvas pane').toBeLessThan(1);
    expectInside(panel, pane);
  });
});
