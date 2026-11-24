import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer } from './helpers';
import { clearAllThreads } from './db-helpers';


test.describe('Explainer in the thread filter panel: desktop layout', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  const openFilterPanel = async (page: import('@playwright/test').Page) => {
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);
    await openThreadDrawer(page);
    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    await filterBtn.click();
    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();
    return filterBtn;
  };

  test('the info icon opens a popover explaining Include deleted, and Escape closes it', async ({ page }) => {
    await openFilterPanel(page);

    const info = page.locator('.thread-filter-panel button[aria-label="About Include deleted"]');
    await expect(info).toBeVisible();
    await expect(page.locator('.explainer-popover')).toHaveCount(0);

    await info.click();
    const popover = page.locator('.explainer-popover');
    await expect(popover).toBeVisible();
    await expect(popover).toHaveAttribute('role', 'dialog');
    await expect(popover.locator('.surface-title')).toHaveText('Include deleted');
    await expect(popover.locator('.explainer-body')).toContainText('(deleted)');
    await expect(popover.getByRole('button', { name: 'Close Include deleted' })).toBeFocused();

    await expect(page.locator('.modal-overlay')).toHaveCount(0);
    const icon = (await info.boundingBox())!;
    const panel = (await popover.boundingBox())!;
    const pane = (await page.locator('.thread-drawer').boundingBox())!;
    const below = panel.y - (icon.y + icon.height);
    const above = icon.y - (panel.y + panel.height);
    expect(Math.min(Math.abs(below), Math.abs(above)), 'panel hangs off the icon').toBeLessThan(12);
    expect(panel.x).toBeGreaterThanOrEqual(pane.x);
    expect(panel.x + panel.width).toBeLessThanOrEqual(pane.x + pane.width + 0.5);

    await page.keyboard.press('Escape');
    await expect(popover).toHaveCount(0);
    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();
  });

  test('tapping the explanation does not toggle the checkbox it is nested inside', async ({ page }) => {
    await openFilterPanel(page);

    const checkbox = page
      .locator('.thread-filter-panel label.thread-filter-option', { hasText: 'Include deleted' })
      .locator('input[type="checkbox"]');
    await expect(checkbox).not.toBeChecked();

    await page.locator('button[aria-label="About Include deleted"]').click();
    const popover = page.locator('.explainer-popover');
    await expect(popover).toBeVisible();

    await popover.locator('.explainer-body p').first().click();
    await expect(popover).toBeVisible();
    await expect(checkbox).not.toBeChecked();

    await popover.getByRole('button', { name: 'Close Include deleted' }).click();
    await expect(popover).toHaveCount(0);
    await expect(checkbox).not.toBeChecked();
  });

  test('a click outside dismisses it without reaching the panel behind, and the icon toggles it', async ({ page }) => {
    await openFilterPanel(page);

    const info = page.locator('button[aria-label="About Include deleted"]');
    const popover = page.locator('.explainer-popover');

    await info.click();
    await expect(popover).toBeVisible();
    await info.click();
    await expect(popover).toHaveCount(0);

    await info.click();
    await expect(popover).toBeVisible();
    await expect(page.locator('html[data-overlay-open]')).toHaveCount(1);

    const canvas = (await page.locator('.split-layout > .pane-content').boundingBox())!;
    await page.mouse.click(canvas.x + canvas.width / 2, canvas.y + canvas.height / 2);
    await expect(popover).toHaveCount(0);

    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Filters');
    await expect(page
      .locator('.thread-filter-panel .thread-filter-option:not(.thread-filter-option-child)', { hasText: 'Lucidos' })
      .locator('input[type="checkbox"]')).toBeChecked();
    await expect(page
      .locator('.thread-filter-panel label.thread-filter-option', { hasText: 'Include deleted' })
      .locator('input[type="checkbox"]')).not.toBeChecked();

    await info.click();
    await expect(popover).toBeVisible();
  });
});
