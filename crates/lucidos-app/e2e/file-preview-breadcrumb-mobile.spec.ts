import { test, expect, type Page } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';

/** The breadcrumb above a file preview, on a phone.
 *
 *  Two promises. The file name never breaks at a hyphen inside it while it fits
 *  on a line. And a folder crumb opens the Files view with that folder's row
 *  just under the fixed header. The target sits between forty sibling folders
 *  on each side, so the scroll can neither skip nor clamp. */

const ROOT = 'artifacts/e2e-breadcrumb';
const TARGET = `${ROOT}/m-deep`;
const SAMPLE_PATH = `${TARGET}/frontmatter-daily-note.md`;
const ROOT_DIR = resolve(WORKSPACE, 'data', ROOT);

async function openSample(page: Page): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'file', params: { file_path: SAMPLE_PATH } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();
  await expect(page.locator('.file-preview-path:visible')).toBeVisible({ timeout: 15_000 });
}

test.describe('the file preview breadcrumb', () => {
  test.beforeAll(() => {
    mkdirSync(resolve(WORKSPACE, 'data', TARGET), { recursive: true });
    writeFileSync(resolve(WORKSPACE, 'data', SAMPLE_PATH), '# Daily note\n');
    for (const prefix of ['a', 'z']) {
      for (let i = 0; i < 40; i++) {
        const dir = resolve(ROOT_DIR, `${prefix}${String(i).padStart(2, '0')}`);
        mkdirSync(dir, { recursive: true });
        writeFileSync(resolve(dir, 'filler.txt'), 'filler\n');
      }
    }
  });

  test.afterAll(() => {
    rmSync(ROOT_DIR, { recursive: true, force: true });
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForEventStream(page);
    await openSample(page);
  });

  test('keeps the file name on one line', async ({ page }) => {
    const lines = await page.locator('.file-preview-path:visible .file-preview-path-name').evaluate((el) => {
      const range = document.createRange();
      range.selectNodeContents(el);
      return new Set([...range.getClientRects()].map(r => Math.round(r.top))).size;
    });
    expect(lines).toBe(1);
  });

  test('opens the Files view with the tapped folder just under the header', async ({ page }) => {
    await page.locator('.file-preview-path:visible button.file-preview-path-folder', { hasText: 'm-deep' }).click();

    const row = page.locator(`.folder-header[data-path="${TARGET}"]:visible`);
    await expect(row).toBeVisible({ timeout: 10_000 });
    // The pane's ::before spacer is the room the fixed header covers.
    await expect.poll(() => row.evaluate((el) => {
      const pane = el.closest('.content-pane-body') as HTMLElement;
      const headerRoom = parseFloat(getComputedStyle(pane, '::before').height) || 0;
      const gap = el.getBoundingClientRect().top - pane.getBoundingClientRect().top - headerRoom;
      return gap > -1 && gap < 2;
    }), { timeout: 5_000 }).toBe(true);
  });
});
