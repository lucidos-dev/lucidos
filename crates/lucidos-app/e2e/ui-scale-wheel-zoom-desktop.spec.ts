
import { test, expect, Page } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';

const UI_SCALE_BUTTON = '[data-search-anchor="appearance:ui-scale"] .settings-option';

async function openScalePanel(page: Page): Promise<void> {
  await navigateToApp(page);
  await waitForEventStream(page);

  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'settings', params: { settings_view: 'appearance' } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();

  const button = page.locator(UI_SCALE_BUTTON);
  await expect(button).toBeVisible({ timeout: 15_000 });
  await button.click();
  await expect(page.locator('.scale-modal')).toBeVisible();
  await expect(page.locator('.scale-modal-label')).toHaveText('100%');
}

async function pinch(page: Page, count: number, deltaY: number): Promise<void> {
  await page.evaluate(({ count, deltaY }) => {
    for (let i = 0; i < count; i++) {
      document.dispatchEvent(new WheelEvent('wheel', {
        deltaY, ctrlKey: true, bubbles: true, cancelable: true,
      }));
    }
  }, { count, deltaY });
}

async function appliedScale(page: Page): Promise<string> {
  return page.evaluate(() =>
    document.documentElement.style.getPropertyValue('--user-ui-scale').trim());
}

test.describe('Desktop UI-scale wheel zoom', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a pinch shorter than one notch moves nothing', async ({ page }) => {
    await openScalePanel(page);

    await pinch(page, 20, -4);
    await page.waitForTimeout(200);
    await expect(page.locator('.scale-modal-label')).toHaveText('100%');
  });

  test('a pinch moves by the distance it travelled, not by its event count', async ({ page }) => {
    await openScalePanel(page);

    await pinch(page, 40, -8);
    await expect(page.locator('.scale-modal-label')).toHaveText('137.5%');
    expect(await appliedScale(page)).toBe('137.5%');
  });

  test('a long burst drains instead of slamming to the clamp', async ({ page }) => {
    await openScalePanel(page);

    await pinch(page, 400, -8);
    await expect(page.locator('.scale-modal-label')).toHaveText('200%');

    await pinch(page, 13, 8);
    await expect(page.locator('.scale-modal-label')).toHaveText('187.5%');
  });

  test('one real wheel notch is one step', async ({ page }) => {
    await openScalePanel(page);

    await page.mouse.move(400, 400);
    await page.keyboard.down('Control');
    await page.mouse.wheel(0, -100);
    await expect(page.locator('.scale-modal-label')).toHaveText('112.5%');

    await page.keyboard.up('Control');
    await expect(page.locator('.scale-modal')).toBeHidden();
    expect(await appliedScale(page)).toBe('112.5%');
  });

  test('a modifier wheel opens the panel from closed, as Cmd +/- does', async ({ page }) => {
    await navigateToApp(page);
    await waitForEventStream(page);
    await expect(page.locator('.scale-modal')).toBeHidden();

    await page.mouse.move(400, 400);
    await page.keyboard.down('Control');
    await page.mouse.wheel(0, -100);
    await expect(page.locator('.scale-modal-label')).toHaveText('112.5%');

    await page.keyboard.up('Control');
    await expect(page.locator('.scale-modal')).toBeHidden();
    expect(await appliedScale(page)).toBe('112.5%');
  });
});
