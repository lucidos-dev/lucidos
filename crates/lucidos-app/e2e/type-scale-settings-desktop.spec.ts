import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';
import { offenders, report } from './typeScaleWalk';

test.use({ viewport: { width: 1280, height: 900 } });

test.describe('Type scale in Settings', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test("every visible text run in What's New is on the scale", async ({ page }) => {
    await navigateToApp(page);
    await waitForEventStream(page);
    const res = await apiRequest(page).post('/api/v1/ui/navigate', {
      headers: { 'content-type': 'application/json' },
      data: { target: 'settings', params: { settings_view: 'whats-new' } },
    });
    expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();

    await page.waitForFunction(
      () => {
        const panel = document.querySelector('.settings-panel');
        if (!panel) return false;
        const rect = panel.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      },
      undefined,
      { timeout: 15_000 }
    );

    const firstRelease = page.locator('.whats-new-row, .whats-new-toggle').first();
    if (await firstRelease.count()) {
      await firstRelease.click();
      await page.waitForTimeout(300);
    }

    const found = await offenders(page);
    expect(found, `off-scale text in What's New:\n${report(found)}`).toEqual([]);
  });
});
