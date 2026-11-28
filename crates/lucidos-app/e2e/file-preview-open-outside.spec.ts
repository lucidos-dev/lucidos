import { test, expect, type Locator, type Page } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import {
  apiRequest, assertHealthy, clickHeaderAction, clickVisibleElement, headerActionOffered,
  navigateToApp, waitForEventStream,
} from './helpers';

const SAMPLE_PATH = 'artifacts/e2e-popout-report.html';
const SAMPLE_FILE = resolve(WORKSPACE, 'data', SAMPLE_PATH);
const POPOUT = '.file-open-in-tab';

async function popoutControl(page: Page): Promise<Locator> {
  const inHeader = page.locator(`${POPOUT}:visible`);
  if (await inHeader.count() > 0) return inHeader.first();
  expect(await clickVisibleElement(page, '.content-header-more')).toBe(true);
  const row = page.locator(`.thread-overflow-item${POPOUT}`);
  await expect(row).toBeVisible({ timeout: 10_000 });
  return row;
}

async function openArtifact(page: Page): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'file', params: { file_path: SAMPLE_PATH } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
  await expect(page.locator('.file-preview-inline:visible iframe').first())
    .toBeVisible({ timeout: 15_000 });
}

test.describe('the previewed file', () => {
  test.beforeAll(() => {
    mkdirSync(resolve(WORKSPACE, 'data/artifacts'), { recursive: true });
    writeFileSync(SAMPLE_FILE, '<!DOCTYPE html>\n<html><body><h1>report</h1></body></html>\n');
  });

  test.afterAll(() => {
    rmSync(SAMPLE_FILE, { force: true });
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForEventStream(page);
  });

  test('can be opened outside the shell, at its own URL', async ({ page }) => {
    await openArtifact(page);

    expect(await headerActionOffered(page, POPOUT)).toBe(true);
    const control = await popoutControl(page);
    await expect(control).toHaveAttribute('href', `/data/${SAMPLE_PATH}`);
    await expect(control).toHaveAttribute('target', '_blank');
  });

  test('offers nothing to open while the inline editor holds the header', async ({ page }) => {
    await openArtifact(page);
    await clickHeaderAction(page, '.file-edit-btn');
    await expect(page.locator('.file-editor-textarea:visible')).toBeVisible({ timeout: 10_000 });

    expect(await headerActionOffered(page, POPOUT, 3_000)).toBe(false);
  });
});
