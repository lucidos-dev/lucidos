import { test, expect, type Page } from './fixtures';
import type { Route } from '@playwright/test';
import { assertHealthy, navigateToApp, uniqueMessage, userMessageBody, waitForVisibleInput } from './helpers';

/** An image still uploading when the page reloads comes back as a chip and
 *  lands, and a Send pressed before the reload still goes. Exercises the
 *  real IndexedDB store, which Vitest cannot reach.
 *  Plan: `docs/plans/2026-10-02-pending-uploads-survive-a-reload.md`.
 *
 *  Service workers are blocked, as in `image-upload-progress.spec.ts`, so
 *  `page.route` sees the upload in WebKit too. */

test.use({ serviceWorkers: 'block' });

const BLOB_ROUTE = '**/api/v1/threads/*/blobs';

/** A real 1x1 PNG: the composer sniffs the bytes before it draws a chip. */
const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
  'base64',
);

async function attachImage(page: Page): Promise<void> {
  await page.locator('.prompt-input-container input[type="file"][accept="image/*"]').first()
    .setInputFiles({ name: 'photo.png', mimeType: 'image/png', buffer: PNG });
}

const pendingChip = (page: Page) => page.locator('.image-preview-pending:visible');
const confirmedImage = (page: Page) => page.locator('.image-preview-item:visible');
const uploadNotice = (page: Page) => page.locator('[data-role="upload-send-notice"]:visible');
const sendButton = (page: Page) => page.locator('button[aria-label="Send message"]:visible').first();

/** How many records one store of the pending-upload database holds. */
function storedCount(page: Page, storeName: 'uploads' | 'queued-upload-sends'): Promise<number> {
  return page.evaluate(async (storeName) => {
    let count = 0;
    for (const { name } of await indexedDB.databases()) {
      if (!name?.startsWith('lucidos-pending-uploads')) continue;
      count += await new Promise<number>((resolve, reject) => {
        const open = indexedDB.open(name);
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          const db = open.result;
          const req = db.transaction(storeName, 'readonly').objectStore(storeName).count();
          req.onsuccess = () => { db.close(); resolve(req.result); };
          req.onerror = () => { db.close(); reject(req.error); };
        };
      });
    }
    return count;
  }, storeName);
}

/** Hold every upload until `release`. A request the reload cut off is gone,
 *  so continuing it fails, and that failure is expected. */
async function holdUploads(page: Page): Promise<() => void> {
  let release!: () => void;
  const held = new Promise<void>((resolve) => { release = resolve; });
  await page.route(BLOB_ROUTE, async (route: Route) => {
    await held;
    await route.continue().catch(() => {});
  });
  return release;
}

/** Type a draft and wait for the engine to store it, so the reload finds it. */
async function typeStoredDraft(page: Page, message: string): Promise<void> {
  const stored = page.waitForResponse((r) =>
    r.url().endsWith('/compose') && r.request().method() === 'PUT' && r.ok());
  const input = await waitForVisibleInput(page);
  await input.fill(message);
  await stored;
}

test.describe('A pending image upload survives a page reload', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('the image comes back as a chip, lands, and its record is cleaned up', async ({ page }) => {
    const release = await holdUploads(page);
    await navigateToApp(page);
    const message = uniqueMessage('upload-reload');
    await typeStoredDraft(page, message);
    await attachImage(page);
    await expect(pendingChip(page)).toHaveCount(1);
    await expect.poll(() => storedCount(page, 'uploads')).toBe(1);

    await page.reload();
    const input = await waitForVisibleInput(page);
    await expect(input).toHaveValue(message);
    await expect(pendingChip(page)).toHaveCount(1);

    release();
    await expect(pendingChip(page)).toHaveCount(0);
    await expect(confirmedImage(page)).toHaveCount(1);

    await sendButton(page).click();
    await expect(userMessageBody(page).filter({ hasText: message })).toHaveCount(1, { timeout: 15_000 });
    await expect.poll(() => storedCount(page, 'uploads')).toBe(0);
  });

  test('a Send pressed before the reload is still queued after it, then sends', async ({ page }) => {
    const release = await holdUploads(page);
    await navigateToApp(page);
    const message = uniqueMessage('upload-reload-queued');
    await typeStoredDraft(page, message);
    await attachImage(page);
    await expect(pendingChip(page)).toHaveCount(1);
    await sendButton(page).click();
    await expect(uploadNotice(page)).toHaveText('Sends when the image finishes uploading');
    await expect.poll(() => storedCount(page, 'uploads')).toBe(1);
    await expect.poll(() => storedCount(page, 'queued-upload-sends')).toBe(1);

    await page.reload();
    await waitForVisibleInput(page);
    await expect(pendingChip(page)).toHaveCount(1);
    await expect(uploadNotice(page)).toHaveText('Sends when the image finishes uploading');

    release();
    await expect(userMessageBody(page).filter({ hasText: message })).toHaveCount(1, { timeout: 15_000 });
    await expect(pendingChip(page)).toHaveCount(0);
    await expect.poll(() => storedCount(page, 'queued-upload-sends')).toBe(0);
  });
});
