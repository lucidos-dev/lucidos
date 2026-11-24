import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, navigateToApp } from './helpers';

test.describe('Harness apiRequest registers a borrowed device id', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a mutating call with an unregistered borrowed page id is not refused', async ({ page }) => {
    await navigateToApp(page);

    const freshId = randomUUID();
    await page.evaluate((id) => localStorage.setItem('lucidos-device-id', id), freshId);

    const res = await apiRequest(page).put('/api/v1/preferences?key=mobile_dynamic_bars', {
      data: { value: 'false' },
    });
    expect(res.ok(), `PUT /api/v1/preferences -> ${res.status()}`).toBeTruthy();
  });
});
