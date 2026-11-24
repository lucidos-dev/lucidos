import { test, expect, type Page } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';

const SAMPLE_PATH = 'artifacts/e2e-context-menu-note.md';
const SAMPLE_FILE = resolve(WORKSPACE, 'data', SAMPLE_PATH);

async function openNote(page: Page): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'file', params: { file_path: SAMPLE_PATH } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
  await expect(page.locator('.file-preview-frame-body:visible')).toBeVisible({ timeout: 15_000 });
}

const menu = (page: Page) => page.locator('.thread-overflow-menu');
const menuItemLabels = (page: Page) => menu(page).locator('[role="menuitem"]').allTextContents();

test.describe('Desktop file-preview context menu', () => {
  test.beforeAll(() => {
    mkdirSync(resolve(WORKSPACE, 'data/artifacts'), { recursive: true });
    writeFileSync(SAMPLE_FILE, [
      '# Context menu note',
      '',
      'Ordinary prose a reader can right-click.',
      '',
      '[a link](https://example.com)',
      '',
    ].join('\n'));
  });

  test.afterAll(() => {
    rmSync(SAMPLE_FILE, { force: true });
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForEventStream(page);
  });

  test('right-clicking ordinary text opens the same actions the header offers', async ({ page }) => {
    await openNote(page);
    const prose = page.getByText('Ordinary prose a reader can right-click.');
    const box = (await prose.boundingBox())!;

    await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2, { button: 'right' });

    await expect(menu(page)).toBeVisible();
    expect(await menuItemLabels(page)).toContain('Copy path');
  });

  test('right-clicking a link defers to the native menu', async ({ page }) => {
    await openNote(page);
    const link = page.getByRole('link', { name: 'a link' });
    const box = (await link.boundingBox())!;

    await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2, { button: 'right' });

    await expect(menu(page)).toHaveCount(0);
  });

  test('right-clicking a selection the reader just made defers to the native menu', async ({ page }) => {
    await openNote(page);
    const prose = page.getByText('Ordinary prose a reader can right-click.');
    const point = await prose.evaluate((el) => {
      const range = document.createRange();
      range.selectNodeContents(el);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
      const rect = range.getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    });

    await page.mouse.click(point.x, point.y, { button: 'right' });

    await expect(menu(page)).toHaveCount(0);
  });

  test('opening and dismissing the menu leaves the preview exactly as it was', async ({ page }) => {
    await openNote(page);
    const prose = page.getByText('Ordinary prose a reader can right-click.');
    const box = (await prose.boundingBox())!;

    await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2, { button: 'right' });
    await expect(menu(page)).toBeVisible();

    await page.keyboard.press('Escape');
    await expect(menu(page)).toHaveCount(0);
    await expect(page.locator('.file-preview-frame-body:visible')).toBeVisible();
    await expect(page.locator('.file-editor-textarea')).toHaveCount(0);
    await expect(prose).toBeVisible();
  });
});
