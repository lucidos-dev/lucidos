import { test, expect, Page } from './fixtures';
import {
  assertHealthy,
  ensureOnThreadPane,
  waitForVisibleInput,
  openFilesPanel,
  waitForVisibleElement,
  clickVisibleElement,
  clickHeaderAction,
  gotoWithRetry,
} from './helpers';

/** Create a markdown data file via the API. Returns the relative data path. */
async function createMdFile(page: Page, name: string, content: string): Promise<string> {
  const path = `artifacts/${name}`;
  const resp = await page.request.put(`/api/v1/data/${path}`, {
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
  await page.request.delete(`/api/v1/data/${path}`);
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

    // Open the file in the preview by clicking it in the folder tree.
    await waitForVisibleElement(page, '.file-item', 15_000);
    expect(await clickVisibleElement(page, '.file-item', fileName)).toBe(true);

    // The rendered markdown preview shows the original body.
    await waitForVisibleElement(page, '.file-preview-content', 10_000);
    await expect(page.locator('.file-preview-content:visible').first()).toContainText('original body');

    // Enter edit mode via the header Edit action (in the ⋯ menu on a narrow row).
    await clickHeaderAction(page, '.file-edit-btn');

    // The editable textarea appears, seeded with the current raw content.
    const textarea = page.locator('.file-editor-textarea:visible').first();
    await expect(textarea).toBeVisible({ timeout: 5_000 });
    await expect(textarea).toHaveValue(/original body/);

    // Replace the content and save.
    const marker = `edited-${Date.now()}`;
    await textarea.fill(`# E2E heading\n\n${marker}\n`);
    expect(await clickVisibleElement(page, '.action-btn-confirm', 'Save')).toBe(true);

    // Editor closes and the rendered preview reflects the new content.
    await expect(page.locator('.file-editor-textarea')).toHaveCount(0, { timeout: 10_000 });
    await expect(page.locator('.file-preview-content:visible').first()).toContainText(marker, { timeout: 10_000 });

    // The write is persisted server-side.
    expect(await readMdFile(page, filePath)).toContain(marker);
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

    // Editor closes; the original content is untouched on disk.
    await expect(page.locator('.file-editor-textarea')).toHaveCount(0, { timeout: 10_000 });
    expect(await readMdFile(page, filePath)).toContain('original body');
  });
});
