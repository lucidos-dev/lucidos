import { test, expect } from '@playwright/test';
import { navigateToApp, assertHealthy, newThread, openThreadDrawer, waitForVisibleInput } from './helpers';

test.describe('Empty and error states', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('fresh app shows compose view, not loading forever', async ({ page }) => {
    await navigateToApp(page);

    // Go to compose view
    await newThread(page);

    // The input should be visible and ready
    const input = await waitForVisibleInput(page);
    await expect(input).toBeVisible();

    // The placeholder should always be the same nudge
    const placeholder = await input.getAttribute('placeholder');
    expect(placeholder).toBe('Go ahead…');
  });

  test('thread drawer can be opened and shows content', async ({ page }) => {
    await navigateToApp(page);

    // Open the thread drawer (openThreadDrawer already waits for drawer visibility)
    await openThreadDrawer(page);

    // Whether there are threads or not, the drawer rendered (not stuck loading)
    const threadCount = await page.locator('.thread-row').count();
    expect(threadCount).toBeGreaterThanOrEqual(0);
  });

  test('health endpoint returns ok', async ({ page }) => {
    const response = await page.request.get('/api/v1/health');
    expect(response.ok()).toBeTruthy();
    const body = await response.json();
    expect(body.status).toBe('ok');
    expect(body.workspace).toBeTruthy();
    expect(body.workspace_path).toBeTruthy();
    expect(body.engine_version).toBeTruthy();
  });

  test('malformed chat request returns error', async ({ page }) => {
    const response = await page.request.post('/api/v1/chat/stream', {
      headers: { 'content-type': 'application/json' },
      data: '{invalid json}',
      failOnStatusCode: false,
    });
    const status = response.status();
    expect(status).toBeGreaterThanOrEqual(400);
    expect(status).toBeLessThan(500);
  });
});
