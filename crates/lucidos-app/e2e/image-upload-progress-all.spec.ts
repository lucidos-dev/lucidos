import { test, expect, type Page } from './fixtures';
import type { Route } from '@playwright/test';
import { assertHealthy, navigateToApp, uniqueMessage, userMessageBody, waitForVisibleInput } from './helpers';


test.use({ serviceWorkers: 'block' });

const BLOB_ROUTE = '**/api/v1/threads/*/blobs';

const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
  'base64',
);

async function attachImage(page: Page): Promise<void> {
  await page.locator('.prompt-input-container input[type="file"][accept="image/*"]').first()
    .setInputFiles({ name: 'photo.png', mimeType: 'image/png', buffer: PNG });
}

const pendingChip = (page: Page) => page.locator('.image-preview-pending:visible');
const uploadNotice = (page: Page) => page.locator('[data-role="upload-send-notice"]:visible');
const sendButton = (page: Page) => page.locator('button[aria-label="Send message"]:visible').first();

test.describe('Image upload progress in the composer', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a slow upload shows progress, and a Send pressed meanwhile waits visibly, then sends', async ({ page }) => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => { release = resolve; });
    await page.route(BLOB_ROUTE, async (route: Route) => {
      await held;
      await route.fallback();
    });
    await navigateToApp(page);
    const message = uniqueMessage('upload-queued');
    const input = await waitForVisibleInput(page);
    await input.fill(message);

    await attachImage(page);
    await expect(pendingChip(page)).toHaveCount(1);
    await expect(pendingChip(page).locator('.upload-overlay[role="progressbar"]')).toBeVisible();

    await sendButton(page).click();
    await expect(uploadNotice(page)).toHaveText('Sends when the image finishes uploading');
    await expect(input).toHaveValue(message);

    release();
    await expect(pendingChip(page)).toHaveCount(0);
    await expect(uploadNotice(page)).toHaveCount(0);
    await expect(userMessageBody(page).filter({ hasText: message })).toHaveCount(1, { timeout: 15_000 });
  });

  test('a failed upload keeps the draft and the image, refuses Send, and Retry lands it', async ({ page }) => {
    let refuse = true;
    await page.route(BLOB_ROUTE, async (route: Route) => {
      if (!refuse) return route.fallback();
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'disk full' }),
      });
    });
    await navigateToApp(page);
    const message = uniqueMessage('upload-failed');
    const input = await waitForVisibleInput(page);
    await input.fill(message);

    await attachImage(page);
    const retry = pendingChip(page).locator('button.upload-retry');
    await expect(retry).toBeVisible();
    await expect(retry).toHaveAttribute('aria-label', 'Upload failed: disk full. Retry');

    await sendButton(page).click();
    await expect(uploadNotice(page)).toHaveText('Not sent: an image failed to upload. Retry it or remove it, then send.');
    await expect(input).toHaveValue(message);
    await expect(userMessageBody(page).filter({ hasText: message })).toHaveCount(0);

    refuse = false;
    await retry.click();
    await expect(pendingChip(page)).toHaveCount(0);
    await expect(uploadNotice(page)).toHaveCount(0);
    await expect(page.locator('.image-preview-item:visible')).toHaveCount(1);

    await sendButton(page).click();
    await expect(userMessageBody(page).filter({ hasText: message })).toHaveCount(1, { timeout: 15_000 });
  });

  test('an upload the engine took lands on its event when the answer never arrives', async ({ page }) => {
    let uploads = 0;
    await page.route(BLOB_ROUTE, async (route: Route) => {
      uploads += 1;
      const headers = await route.request().allHeaders();
      delete headers['content-type'];
      delete headers['content-length'];
      const stored = await route.fetch({
        headers,
        multipart: { file: { name: 'photo.png', mimeType: 'image/png', buffer: PNG } },
      });
      expect(stored.status(), 'the engine refused the replayed upload').toBe(201);
    });
    await navigateToApp(page);
    const message = uniqueMessage('upload-answer-lost');
    const input = await waitForVisibleInput(page);
    await input.fill(message);

    await attachImage(page);
    await sendButton(page).click();

    await expect(pendingChip(page)).toHaveCount(0, { timeout: 10_000 });
    await expect(userMessageBody(page).filter({ hasText: message })).toHaveCount(1, { timeout: 15_000 });
    expect(uploads).toBe(1);
  });
});
