import { test, expect } from '@playwright/test';
import { assertHealthy } from './helpers';

test.describe('Error display states', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('malformed chat stream request shows error response', async ({ page }) => {
    // Send a request with missing required fields
    const resp = await page.request.post('/api/chat/stream', {
      headers: { 'content-type': 'application/json' },
      data: JSON.stringify({ text: '' }),
      failOnStatusCode: false,
    });
    expect(resp.status()).toBeGreaterThanOrEqual(400);
    expect(resp.status()).toBeLessThan(500);
  });

  test('chat stream with invalid thread_id returns error', async ({ page }) => {
    const resp = await page.request.post('/api/chat/stream', {
      headers: { 'content-type': 'application/json' },
      data: JSON.stringify({ text: 'hello', thread_id: 'not-a-valid-uuid' }),
      failOnStatusCode: false,
    });
    expect(resp.status()).toBeGreaterThanOrEqual(400);
    expect(resp.status()).toBeLessThan(500);
  });

  test('non-existent API endpoint returns 404', async ({ page }) => {
    const resp = await page.request.get('/api/nonexistent', {
      failOnStatusCode: false,
    });
    expect(resp.status()).toBe(404);
  });

  test('changes API with invalid change ID returns error', async ({ page }) => {
    const resp = await page.request.post('/api/changes/not-a-uuid/apply', {
      failOnStatusCode: false,
    });
    expect(resp.status()).toBeGreaterThanOrEqual(400);
  });
});
