import { test, expect, Page } from './fixtures';
import {
  apiRequest, assertHealthy, clickHeaderAction, clickVisibleElement, ensureOnThreadPane,
  gotoWithRetry, openFilesPanel, waitForVisibleElement, waitForVisibleInput,
} from './helpers';

async function createMdFile(page: Page, name: string, content: string): Promise<string> {
  const path = `artifacts/${name}`;
  const resp = await apiRequest(page).put(`/api/v1/data/${path}`, {
    headers: { 'Content-Type': 'text/plain' },
    data: content,
  });
  expect(resp.ok()).toBeTruthy();
  return path;
}

async function readMdFile(page: Page, path: string): Promise<string> {
  const resp = await page.request.get(`/api/v1/data/${path}`);
  expect(resp.ok()).toBeTruthy();
  return resp.text();
}

async function deleteMdFile(page: Page, path: string): Promise<void> {
  await apiRequest(page).delete(`/api/v1/data/${path}`);
}

test.describe('File preview inline editing', () => {
  let fileName: string;
  let filePath: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    fileName = `e2e-edit-${Date.now()}.md`;
    filePath = await createMdFile(page, fileName, '# E2E heading\n\noriginal body\n');

    await gotoWithRetry(page, '/');
    await page.waitForFunction(() =>
      document.querySelector('#app')?.childElementCount! > 0,
      undefined, { timeout: 30_000 },
    );
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);
  });

  test.afterEach(async ({ page }) => {
    if (filePath) await deleteMdFile(page, filePath);
  });

  test('edit a markdown file and save', async ({ page }) => {
    await openFilesPanel(page);

    await waitForVisibleElement(page, '.file-item', 15_000);
    expect(await clickVisibleElement(page, '.file-item', fileName)).toBe(true);

    await waitForVisibleElement(page, '.file-preview-content', 10_000);
    await expect(page.locator('.file-preview-content:visible').first()).toContainText('original body');

    await clickHeaderAction(page, '.file-edit-btn');

    const textarea = page.locator('.file-editor-textarea:visible').first();
    await expect(textarea).toBeVisible({ timeout: 5_000 });
    await expect(textarea).toHaveValue(/original body/);

    const toolbarBtn = (text: string) =>
      page.locator('.file-editor-toolbar button:visible').filter({ hasText: text }).first();
    await expect(toolbarBtn('Close')).toBeVisible();
    await expect(page.locator('.file-editor-toolbar button:visible').filter({ hasText: 'Save' }))
      .toHaveCount(0);

    const marker = `edited-${Date.now()}`;
    await textarea.fill(`# E2E heading\n\n${marker}\n`);
    await expect(toolbarBtn('Save')).toBeEnabled();
    await toolbarBtn('Save').click();

    await expect(toolbarBtn('Close')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.file-editor-textarea:visible').first()).toBeVisible();

    expect(await readMdFile(page, filePath)).toContain(marker);

    await toolbarBtn('Close').click();
    await expect(page.locator('.file-editor-textarea')).toHaveCount(0, { timeout: 10_000 });
    await expect(page.locator('.file-preview-content:visible').first()).toContainText(marker, { timeout: 10_000 });
  });

  test('cancel discards the draft', async ({ page }) => {
    await openFilesPanel(page);

    await waitForVisibleElement(page, '.file-item', 15_000);
    expect(await clickVisibleElement(page, '.file-item', fileName)).toBe(true);

    await clickHeaderAction(page, '.file-edit-btn');

    const textarea = page.locator('.file-editor-textarea:visible').first();
    await expect(textarea).toBeVisible({ timeout: 5_000 });
    await textarea.fill('# E2E heading\n\nthrow-away edit\n');

    expect(await clickVisibleElement(page, '.action-btn-danger', 'Cancel')).toBe(true);

    await expect(page.locator('.file-editor-textarea')).toHaveCount(0, { timeout: 10_000 });
    expect(await readMdFile(page, filePath)).toContain('original body');
  });
});
